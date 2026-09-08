//! Release check, download, verification and binary swap for the Windows
//! tray. Runs on the worker thread; the pure decisions (version compare,
//! asset selection, sha256, the rename dance) live in `crate::update` so
//! they are unit-tested on every OS. This file is only the glue around
//! `reqwest` and the process's own paths.

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
