//! Release check, download, verification and binary swap for the Windows
//! tray.
//!
//! Compiled on every OS so Linux CI exercises the release-check logic; only
//! the Windows host calls the install path, hence the `dead_code` allowance
//! off Windows. Runs on the worker thread; the pure decisions (version compare,
//! asset selection, sha256, the rename dance) live in `crate::update` so
//! they are unit-tested on every OS. This file is only the glue around
//! `reqwest` and the process's own paths.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::update::{
    BINARIES, Download, MAX_ASSET_BYTES, Release, current_arch, is_newer, latest_release_url,
    parse_release, parse_sha256_sidecar, select_downloads, stage_swap, staging_dir, verify_sha256,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// A sha256 sidecar is one line; anything bigger is not ours.
const MAX_SIDECAR_BYTES: usize = 4 * 1024;
/// GitHub asks for a User-Agent; naming the version helps them and us.
const USER_AGENT: &str = concat!("ai-usagebar-tray/", env!("CARGO_PKG_VERSION"));

pub fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(DOWNLOAD_TIMEOUT)
        .connect_timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("could not build the update HTTP client: {e}"))
}

/// Ask GitHub for the latest release. `Ok(None)` means "up to date" (or a
/// prerelease/draft, which the tray never offers).
pub async fn check(
    client: &reqwest::Client,
    current_version: &str,
) -> Result<Option<Release>, String> {
    let Some(url) = latest_release_url() else {
        return Err("this build names no GitHub repository to check".into());
    };
    check_at(client, &url, current_version).await
}

/// [`check`] against an explicit URL. The test seam: `latest_release_url` is a
/// compile-time constant pointing at the real repository, so without this a
/// test of the response handling would have to reach GitHub.
pub async fn check_at(
    client: &reqwest::Client,
    url: &str,
    current_version: &str,
) -> Result<Option<Release>, String> {
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("release check failed: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("release check returned HTTP {}", status.as_u16()));
    }
    let body = response
        .text()
        .await
        .map_err(|e| format!("release check body unreadable: {e}"))?;
    let release = match parse_release(&body) {
        Ok(release) => release,
        Err(reason) if reason.contains("prerelease") || reason.contains("draft") => {
            return Ok(None);
        }
        Err(reason) => return Err(reason),
    };
    if is_newer(current_version, &release.version) {
        Ok(Some(release))
    } else {
        Ok(None)
    }
}

/// Download every asset the release ships for this machine, verify each
/// against its sha256 sidecar, then swap the binaries beside the running
/// exe. Returns the path of the new tray exe to relaunch.
pub async fn install(client: &reqwest::Client, release: &Release) -> Result<PathBuf, String> {
    if cfg!(debug_assertions) {
        return Err("This is a development build; rebuild from source to update.".into());
    }
    let install_dir = install_dir()?;
    let cache_root = crate::cache::xdg_cache_dir()
        .map_err(|e| e.to_string())?
        .join("ai-usagebar");
    let staging = staging_dir(&cache_root, &release.version);
    std::fs::create_dir_all(&staging)
        .map_err(|e| format!("could not create {}: {e}", staging.display()))?;

    let downloads = select_downloads(release, current_arch())?;
    let mut staged = Vec::with_capacity(downloads.len());
    for download in &downloads {
        let path = fetch_and_verify(client, download, &staging).await?;
        staged.push((format!("{}.exe", download.binary), path));
    }
    stage_swap(&install_dir, &staged)?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(install_dir.join(format!("{}.exe", BINARIES[0])))
}

async fn fetch_and_verify(
    client: &reqwest::Client,
    download: &Download,
    staging: &Path,
) -> Result<PathBuf, String> {
    let sidecar = fetch_bytes(client, &download.sha256.url, MAX_SIDECAR_BYTES as u64).await?;
    let expected = parse_sha256_sidecar(&String::from_utf8_lossy(&sidecar))?;
    let bytes = fetch_bytes(client, &download.exe.url, MAX_ASSET_BYTES).await?;
    let path = staging.join(&download.exe.name);
    crate::cache::atomic_write(&path, &bytes).map_err(|e| e.to_string())?;
    verify_sha256(&path, &expected)?;
    Ok(path)
}

async fn fetch_bytes(client: &reqwest::Client, url: &str, cap: u64) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err("refusing a non-HTTPS download".into());
    }
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("download returned HTTP {}", status.as_u16()));
    }
    if let Some(len) = response.content_length()
        && len > cap
    {
        return Err(format!(
            "download is {len} bytes, above the {cap} byte limit"
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("download body failed: {e}"))?;
    if bytes.len() as u64 > cap {
        return Err(format!(
            "download is {} bytes, above the {cap} byte limit",
            bytes.len()
        ));
    }
    Ok(bytes.to_vec())
}

/// Directory of the running exe; the update replaces siblings there.
pub fn install_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current exe unknown: {e}"))?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "current exe has no parent directory".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_json(tag: &str, prerelease: bool) -> String {
        format!(
            r#"{{"tag_name":"{tag}","prerelease":{prerelease},"draft":false,
                "html_url":"https://github.com/akitaonrails/ai-usagebar/releases/tag/{tag}",
                "assets":[
                  {{"name":"ai-usagebar-tray-windows-x86_64.exe","browser_download_url":"https://github.com/x/y/a.exe","size":10}},
                  {{"name":"ai-usagebar-tray-windows-x86_64.exe.sha256","browser_download_url":"https://github.com/x/y/a.exe.sha256","size":80}}
                ]}}"#
        )
    }

    /// A newer stable release is offered.
    #[tokio::test]
    async fn a_newer_release_is_reported() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/latest")
            .with_status(200)
            .with_body(release_json("v9.9.9", false))
            .create_async()
            .await;
        let found = check_at(
            &http_client().unwrap(),
            &format!("{}/latest", server.url()),
            "1.0.0",
        )
        .await
        .unwrap();
        assert_eq!(found.map(|r| r.version), Some("9.9.9".to_string()));
    }

    /// The tray must never offer a prerelease: `parse_release` refuses it and
    /// `check` turns that into "nothing to do", not an error the UI shows.
    #[tokio::test]
    async fn a_prerelease_is_not_an_update_and_not_an_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/latest")
            .with_status(200)
            .with_body(release_json("v9.9.9", true))
            .create_async()
            .await;
        let found = check_at(
            &http_client().unwrap(),
            &format!("{}/latest", server.url()),
            "1.0.0",
        )
        .await
        .unwrap();
        assert!(found.is_none(), "{found:?}");
    }

    /// The same version is not an update.
    #[tokio::test]
    async fn the_running_version_is_not_an_update() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/latest")
            .with_status(200)
            .with_body(release_json("v1.0.0", false))
            .create_async()
            .await;
        let found = check_at(
            &http_client().unwrap(),
            &format!("{}/latest", server.url()),
            "1.0.0",
        )
        .await
        .unwrap();
        assert!(found.is_none(), "{found:?}");
    }

    /// A rate-limited or broken API is an error naming the status, and never a
    /// silent "up to date" that would hide a stuck updater.
    #[tokio::test]
    async fn a_failed_release_check_reports_the_status() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/latest")
            .with_status(403)
            .with_body("rate limited")
            .create_async()
            .await;
        let err = check_at(
            &http_client().unwrap(),
            &format!("{}/latest", server.url()),
            "1.0.0",
        )
        .await
        .unwrap_err();
        assert!(err.contains("403"), "{err}");
        // The upstream body may name the account; it must not reach the UI.
        assert!(!err.contains("rate limited"), "{err}");
    }
}
