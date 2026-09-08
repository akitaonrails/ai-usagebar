//! Self-update support for the Windows tray: release discovery, asset
//! selection, checksum verification, and the in-place binary swap.
//!
//! Linux installs get new versions from the AUR, Nix, or cargo-binstall; a
//! Windows install is a zip the user unpacked by hand, so nothing would ever
//! tell it a newer release exists. The tray host polls GitHub's *latest*
//! release once per [`CHECK_INTERVAL`], downloads the per-binary `.exe`
//! assets the release workflow publishes, checks each against its `.sha256`
//! sidecar, and swaps the verified files into the install directory.
//!
//! Everything in this module is pure or takes an explicit path, so the whole
//! flow is unit-tested against a temp directory and never touches the network
//! or a real install. The tray host owns the HTTP calls and the UI; this
//! module owns every decision the host must not get wrong twice:
//!
//! - [`parse_release`] / [`is_newer`]: a draft or prerelease is never an
//!   update, and only a strict `X.Y.Z` compares — "newer" must never be a
//!   string comparison, or `1.9.10` loses to `1.10.0`.
//! - [`select_downloads`]: the tray binary is the one doing the updating, so
//!   it is mandatory; the CLI and TUI are nice-to-have and a release that
//!   ships without one of them still updates the tray.
//! - [`verify_sha256`]: nothing lands in the install directory unverified.
//! - [`stage_swap`] / [`sweep_old`]: Windows refuses to overwrite a running
//!   executable but happily lets it be *renamed*, so the live exe becomes
//!   `<name>.old`, the staged file takes its place, and the next start
//!   sweeps the `.old` files once no process holds them any more.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;

/// The repository the running binary was built from, as `Cargo.toml`'s
/// `repository` field says. A fork that builds its own tray therefore
/// updates from its own releases without touching code.
pub const SOURCE_REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// GitHub's "latest" is the newest non-draft, non-prerelease release, which
/// is exactly the set the tray may install. Polled once per interval.
pub fn latest_release_url() -> Option<String> {
    latest_release_url_for(SOURCE_REPOSITORY)
}

/// `https://github.com/<owner>/<name>[.git][/]` → the releases/latest API URL.
/// Anything that is not a GitHub repository yields `None`, and the tray
/// reports that it cannot check rather than asking a random host.
pub fn latest_release_url_for(repository: &str) -> Option<String> {
    let path = repository
        .trim()
        .strip_prefix("https://github.com/")?
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let (owner, name) = path.split_once('/')?;
    let valid = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !valid(owner) || !valid(name) || name.contains('/') {
        return None;
    }
    Some(format!(
        "https://api.github.com/repos/{owner}/{name}/releases/latest"
    ))
}

/// Once an hour: releases are rare, and the unauthenticated GitHub API
/// allows sixty requests an hour per IP — one of them is ours.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// A release exe is a few MiB. Anything reporting more than this is not one
/// of ours and is refused before a single byte is downloaded.
pub const MAX_ASSET_BYTES: u64 = 50 * 1024 * 1024;

/// The three Windows binaries, tray first: it is the one that must update
/// (it runs the updater), the other two are optional extras.
pub const BINARIES: [&str; 3] = ["ai-usagebar-tray", "ai-usagebar", "ai-usagebar-tui"];

/// Longest `html_url` or asset name accepted from the release JSON. Real
/// values are well under a hundred characters; anything longer is not
/// something we want to log, display, or join into a path.
const MAX_FIELD_LEN: usize = 512;

/// One downloadable file of a release. `url` is the `browser_download_url`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub url: String,
}

/// The parts of a GitHub release the updater acts on. `version` is bare
/// (`1.11.0`, never `v1.11.0`) so it compares with `CARGO_PKG_VERSION`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub assets: Vec<Asset>,
    pub html_url: String,
    pub version: String,
}

/// The exe/sidecar pair for one binary, as found in a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub binary: &'static str,
    pub exe: Asset,
    pub sha256: Asset,
}

/// Wire shape of `GET /releases/latest` — only the fields we read. Every
/// field but `tag_name` defaults so a schema drift in something we ignore
/// cannot break the update check.
#[derive(Deserialize)]
struct WireRelease {
    #[serde(default)]
    assets: Vec<WireAsset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    prerelease: bool,
    tag_name: Option<String>,
}

#[derive(Deserialize)]
struct WireAsset {
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: u64,
}

/// Parse a release response into what the updater needs.
///
/// A draft or prerelease is an error rather than "not newer": the tray shows
/// the reason, and a silent skip would hide a mis-tagged release forever.
/// Assets are filtered, not rejected wholesale — one odd file in a release
/// must not block the update — but an asset whose name could escape the
/// staging directory (a separator, `..`) is dropped, as is one served from
/// anywhere but HTTPS. `html_url` is the "open release notes" link, which
/// only ever points at GitHub; anything else becomes an empty string so the
/// tray simply shows no link.
pub fn parse_release(json: &str) -> std::result::Result<Release, String> {
    let wire: WireRelease =
        serde_json::from_str(json).map_err(|error| format!("release JSON: {error}"))?;
    if wire.draft || wire.prerelease {
        return Err("prerelease/draft release".to_string());
    }
    let tag = wire
        .tag_name
        .ok_or_else(|| "release JSON has no tag_name".to_string())?;
    let version = tag.strip_prefix('v').unwrap_or(&tag);
    if parse_version(version).is_none() {
        return Err(format!("release tag {tag:?} is not vX.Y.Z"));
    }
    let html_url = if wire.html_url.starts_with("https://github.com/")
        && wire.html_url.len() <= MAX_FIELD_LEN
    {
        wire.html_url
    } else {
        String::new()
    };
    let assets = wire
        .assets
        .into_iter()
        .filter(|asset| asset_name_is_safe(&asset.name))
        .filter(|asset| {
            asset.browser_download_url.starts_with("https://")
                && asset.browser_download_url.len() <= MAX_FIELD_LEN
        })
        .map(|asset| Asset {
            name: asset.name,
            size: asset.size,
            url: asset.browser_download_url,
        })
        .collect();
    Ok(Release {
        assets,
        html_url,
        version: version.to_string(),
    })
}

/// A name is joined onto the staging and install directories verbatim, so it
/// must be a single plain component.
fn asset_name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_FIELD_LEN
        && !name.contains(['/', '\\'])
        && !name.contains("..")
        && name != "."
}

/// Strict `X.Y.Z` with numeric components. Pre-release suffixes, build
/// metadata, and two-part versions are all "not a version" here: the release
/// workflow only ever tags `vX.Y.Z`, so anything else is not one of ours.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Numeric semver comparison. A malformed side is `false`: an update the
/// tray cannot reason about is not an update it should offer.
pub fn is_newer(current: &str, candidate: &str) -> bool {
    match (parse_version(current), parse_version(candidate)) {
        (Some(current), Some(candidate)) => candidate > current,
        _ => false,
    }
}

/// The architecture suffix the release workflow uses in asset names.
/// `"unknown"` selects nothing and the tray reports that instead of
/// installing the wrong binary.
pub fn current_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    }
}

/// `{binary}-windows-{arch}.exe` — the bare-exe asset naming in
/// `.github/workflows/release.yml`. Its sidecar is this plus `.sha256`.
pub fn asset_name(binary: &str, arch: &str) -> String {
    format!("{binary}-windows-{arch}.exe")
}

/// Pair each binary with its exe and sidecar, tray first.
///
/// The tray pair is required: a release without it cannot update the thing
/// that is updating, so the whole check fails loudly. The CLI and TUI pairs
/// are skipped when either half is missing — a partial release still updates
/// the tray. A zero-byte or oversized exe is an error for every binary: that
/// is a broken release, not an optional one, and installing a subset would
/// leave a version mix on disk.
pub fn select_downloads(
    release: &Release,
    arch: &str,
) -> std::result::Result<Vec<Download>, String> {
    let find = |name: &str| release.assets.iter().find(|asset| asset.name == name);
    let mut downloads = Vec::with_capacity(BINARIES.len());
    for binary in BINARIES {
        let required = binary == BINARIES[0];
        let exe_name = asset_name(binary, arch);
        let sidecar_name = format!("{exe_name}.sha256");
        let (exe, sha256) = match (find(&exe_name), find(&sidecar_name)) {
            (Some(exe), Some(sha256)) => (exe, sha256),
            _ if required => {
                return Err(format!(
                    "release {} has no {exe_name} + {sidecar_name} pair",
                    release.version
                ));
            }
            _ => continue,
        };
        if exe.size == 0 {
            return Err(format!("{exe_name} is empty"));
        }
        if exe.size > MAX_ASSET_BYTES {
            return Err(format!(
                "{exe_name} is {} bytes, over the {MAX_ASSET_BYTES}-byte limit",
                exe.size
            ));
        }
        downloads.push(Download {
            binary,
            exe: exe.clone(),
            sha256: sha256.clone(),
        });
    }
    Ok(downloads)
}

/// The sidecar the release workflow writes is `"<hex>  <name>"` (the
/// `sha256sum` format, so `sha256sum -c` works on Linux too); accept a bare
/// digest as well. Folded to lowercase so callers compare bytes.
pub fn parse_sha256_sidecar(text: &str) -> std::result::Result<String, String> {
    let digest = text
        .split_whitespace()
        .next()
        .ok_or_else(|| "empty sha256 sidecar".to_string())?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("sha256 sidecar is not a 64-hex digest: {digest:?}"));
    }
    Ok(digest.to_ascii_lowercase())
}

/// Stream the file through SHA-256 and compare with `expected_hex`
/// (case-insensitive). Streamed rather than read whole: the buffer is a
/// fixed 64 KiB whatever the download turned out to be.
pub fn verify_sha256(path: &Path, expected_hex: &str) -> std::result::Result<(), String> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hex_lower(&hasher.finalize());
    if actual == expected_hex.to_ascii_lowercase() {
        Ok(())
    } else {
        Err(format!(
            "sha256 mismatch for {}: expected {expected_hex}, got {actual}",
            path.display()
        ))
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// One entry [`stage_swap`] has completed, kept so a later failure can undo it.
struct Swapped {
    dest: PathBuf,
    old: Option<PathBuf>,
    source: PathBuf,
}

/// Move verified staged files into `install_dir`, one `(file name, staged
/// path)` per binary, returning the paths written.
///
/// Every staged path is checked up front so a missing download fails before
/// anything on disk moves. Per entry: an existing `install_dir/<name>` is
/// renamed to `<name>.old` (a stale `.old` from an earlier update is removed
/// first) and the staged file is renamed into place — copy + remove when the
/// staging directory sits on another volume and `rename` refuses. On any
/// failure the entries already swapped are rolled back best-effort: the new
/// file goes back to its staged path and the `.old` returns to its name, so
/// the install never ends up half of one version and half of another.
pub fn stage_swap(
    install_dir: &Path,
    staged: &[(String, PathBuf)],
) -> std::result::Result<Vec<PathBuf>, String> {
    for (name, source) in staged {
        if !asset_name_is_safe(name) {
            return Err(format!("refusing to install a file named {name:?}"));
        }
        if !source.is_file() {
            return Err(format!(
                "staged file {} for {name} does not exist",
                source.display()
            ));
        }
    }
    let mut done: Vec<Swapped> = Vec::with_capacity(staged.len());
    for (name, source) in staged {
        match swap_one(install_dir, name, source) {
            Ok(swapped) => done.push(swapped),
            Err(error) => {
                rollback(&done);
                return Err(error);
            }
        }
    }
    Ok(done.into_iter().map(|swapped| swapped.dest).collect())
}

fn swap_one(install_dir: &Path, name: &str, source: &Path) -> std::result::Result<Swapped, String> {
    let dest = install_dir.join(name);
    let old_path = install_dir.join(format!("{name}.old"));
    let mut old = None;
    if dest.exists() {
        if old_path.exists() {
            fs::remove_file(&old_path)
                .map_err(|error| format!("remove stale {}: {error}", old_path.display()))?;
        }
        fs::rename(&dest, &old_path).map_err(|error| {
            format!(
                "rename {} to {}: {error}",
                dest.display(),
                old_path.display()
            )
        })?;
        old = Some(old_path);
    }
    if let Err(error) = move_file(source, &dest) {
        let _ = fs::remove_file(&dest);
        if let Some(old) = &old {
            let _ = fs::rename(old, &dest);
        }
        return Err(error);
    }
    Ok(Swapped {
        dest,
        old,
        source: source.to_path_buf(),
    })
}

/// `rename`, falling back to copy + remove for a cross-volume move. A
/// half-written copy is removed so the destination is never a truncated exe.
fn move_file(from: &Path, to: &Path) -> std::result::Result<(), String> {
    let Err(rename_error) = fs::rename(from, to) else {
        return Ok(());
    };
    if let Err(copy_error) = fs::copy(from, to) {
        let _ = fs::remove_file(to);
        return Err(format!(
            "move {} to {}: rename failed ({rename_error}), copy failed ({copy_error})",
            from.display(),
            to.display()
        ));
    }
    let _ = fs::remove_file(from);
    Ok(())
}

fn rollback(done: &[Swapped]) {
    for swapped in done.iter().rev() {
        if move_file(&swapped.dest, &swapped.source).is_err() {
            let _ = fs::remove_file(&swapped.dest);
        }
        if let Some(old) = &swapped.old {
            let _ = fs::rename(old, &swapped.dest);
        }
    }
}

/// Delete the `*.exe.old` files a previous [`stage_swap`] left behind and
/// return how many went. A file still held by a process that has not exited
/// yet stays for the next sweep; nothing else in the directory is touched.
pub fn sweep_old(install_dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(install_dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".exe.old"))
        .filter(|entry| entry.path().is_file())
        .filter(|entry| fs::remove_file(entry.path()).is_ok())
        .count()
}

/// What the tray remembers between checks: when it last asked GitHub, so a
/// restart does not re-poll, and which version the user dismissed, so the
/// same release is not offered again until a newer one appears.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateState {
    #[serde(default)]
    pub last_check_ms: i64,
    #[serde(default)]
    pub snoozed_version: Option<String>,
}

impl UpdateState {
    /// Missing or unreadable state is the default: the worst case is one
    /// extra poll and one re-shown prompt, which is the safe direction for a
    /// corrupt sidecar.
    pub fn load_at(path: &Path) -> UpdateState {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Atomic write (tempfile + rename), creating the parent directory.
    pub fn save_at(&self, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self)?;
        crate::cache::atomic_write(path, &bytes)
    }
}

/// `<cache dir>/ai-usagebar/update.json` — beside the vendor caches and
/// `detect.json`, because it is derived state that is safe to delete.
pub fn default_state_path() -> Result<PathBuf> {
    Ok(crate::cache::xdg_cache_dir()?
        .join("ai-usagebar")
        .join("update.json"))
}

/// Where downloads for one version are staged: `<cache_root>/updates/<version>`.
/// Per version, so an interrupted download of one release never mixes with
/// the next, and the whole directory can be removed after a swap.
pub fn staging_dir(cache_root: &Path, version: &str) -> PathBuf {
    cache_root.join("updates").join(version)
}

#[cfg(test)]
mod tests {
    #[test]
    fn latest_release_url_follows_the_cargo_repository_field() {
        assert_eq!(
            super::latest_release_url_for("https://github.com/akitaonrails/ai-usagebar"),
            Some("https://api.github.com/repos/akitaonrails/ai-usagebar/releases/latest".into())
        );
        assert_eq!(
            super::latest_release_url_for("https://github.com/djalmajr/ai-usagebar.git/"),
            Some("https://api.github.com/repos/djalmajr/ai-usagebar/releases/latest".into())
        );
        assert_eq!(
            super::latest_release_url_for("https://gitlab.com/x/y"),
            None
        );
        assert_eq!(
            super::latest_release_url_for("https://github.com/only-owner"),
            None
        );
        assert_eq!(
            super::latest_release_url_for("https://github.com/o/n/extra"),
            None
        );
        assert_eq!(
            super::latest_release_url_for("https://github.com/o/n%20e"),
            None
        );
        // The build we are in points somewhere valid.
        assert!(
            super::latest_release_url().is_some(),
            "{}",
            super::SOURCE_REPOSITORY
        );
    }

    use super::*;
    use tempfile::TempDir;

    const RELEASE_FIXTURE: &str = r#"{
  "url": "https://api.github.com/repos/akitaonrails/ai-usagebar/releases/300000",
  "html_url": "https://github.com/akitaonrails/ai-usagebar/releases/tag/v1.11.0",
  "id": 300000,
  "tag_name": "v1.11.0",
  "target_commitish": "main",
  "name": "v1.11.0",
  "draft": false,
  "prerelease": false,
  "created_at": "2026-09-01T12:00:00Z",
  "published_at": "2026-09-01T12:05:00Z",
  "assets": [
    {
      "name": "ai-usagebar-linux-x86_64.tar.gz",
      "size": 9000000,
      "content_type": "application/gzip",
      "browser_download_url": "https://github.com/akitaonrails/ai-usagebar/releases/download/v1.11.0/ai-usagebar-linux-x86_64.tar.gz"
    },
    {
      "name": "ai-usagebar-tray-windows-x86_64.exe",
      "size": 6100000,
      "content_type": "application/octet-stream",
      "browser_download_url": "https://github.com/akitaonrails/ai-usagebar/releases/download/v1.11.0/ai-usagebar-tray-windows-x86_64.exe"
    },
    {
      "name": "ai-usagebar-tray-windows-x86_64.exe.sha256",
      "size": 102,
      "content_type": "application/octet-stream",
      "browser_download_url": "https://github.com/akitaonrails/ai-usagebar/releases/download/v1.11.0/ai-usagebar-tray-windows-x86_64.exe.sha256"
    },
    {
      "name": "../evil.exe",
      "size": 10,
      "browser_download_url": "https://github.com/akitaonrails/ai-usagebar/releases/download/v1.11.0/evil.exe"
    },
    {
      "name": "plain-http.exe",
      "size": 10,
      "browser_download_url": "http://example.com/plain-http.exe"
    }
  ]
}"#;

    fn asset(name: &str, size: u64) -> Asset {
        Asset {
            name: name.to_string(),
            size,
            url: format!(
                "https://github.com/akitaonrails/ai-usagebar/releases/download/v1.11.0/{name}"
            ),
        }
    }

    fn pair(binary: &str, size: u64) -> [Asset; 2] {
        let exe = asset_name(binary, "x86_64");
        [asset(&exe, size), asset(&format!("{exe}.sha256"), 100)]
    }

    fn release_with(assets: Vec<Asset>) -> Release {
        Release {
            assets,
            html_url: String::new(),
            version: "1.11.0".to_string(),
        }
    }

    fn full_release() -> Release {
        release_with(
            BINARIES
                .iter()
                .flat_map(|binary| pair(binary, 5_000_000))
                .collect(),
        )
    }

    #[test]
    fn parse_release_reads_version_url_and_safe_assets() {
        let release = parse_release(RELEASE_FIXTURE).unwrap();

        assert_eq!(release.version, "1.11.0");
        assert_eq!(
            release.html_url,
            "https://github.com/akitaonrails/ai-usagebar/releases/tag/v1.11.0"
        );
        let names: Vec<&str> = release
            .assets
            .iter()
            .map(|asset| asset.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "ai-usagebar-linux-x86_64.tar.gz",
                "ai-usagebar-tray-windows-x86_64.exe",
                "ai-usagebar-tray-windows-x86_64.exe.sha256",
            ],
            "the traversal name and the plain-http asset are dropped"
        );
        let tray = &release.assets[1];
        assert_eq!(tray.size, 6_100_000);
        assert_eq!(
            tray.url,
            "https://github.com/akitaonrails/ai-usagebar/releases/download/v1.11.0/ai-usagebar-tray-windows-x86_64.exe"
        );
    }

    #[test]
    fn parse_release_accepts_a_bare_tag_and_tolerates_missing_assets() {
        let release =
            parse_release(r#"{"tag_name": "1.11.0", "html_url": "https://evil.example/x"}"#)
                .unwrap();

        assert_eq!(release.version, "1.11.0");
        assert!(release.assets.is_empty());
        assert_eq!(release.html_url, "", "a non-GitHub link is blanked");
    }

    #[test]
    fn parse_release_rejects_prerelease_draft_and_malformed_tags() {
        let prerelease = r#"{"tag_name": "v1.11.0", "prerelease": true}"#;
        assert_eq!(
            parse_release(prerelease).unwrap_err(),
            "prerelease/draft release"
        );

        let draft = r#"{"tag_name": "v1.11.0", "draft": true}"#;
        assert_eq!(
            parse_release(draft).unwrap_err(),
            "prerelease/draft release"
        );

        for tag in ["v1.11", "v1.11.0-rc1", "nightly", "v1.11.0.1", ""] {
            let json = format!(r#"{{"tag_name": "{tag}"}}"#);
            assert!(parse_release(&json).is_err(), "{tag:?} parsed");
        }
        assert!(parse_release(r#"{"draft": false}"#).is_err(), "no tag_name");
        assert!(parse_release("not json").is_err());
    }

    #[test]
    fn is_newer_compares_numerically() {
        assert!(is_newer("1.10.0", "1.11.0"));
        assert!(!is_newer("1.11.0", "1.11.0"), "equal is not newer");
        assert!(!is_newer("1.11.0", "1.10.0"), "older is not newer");
        assert!(is_newer("1.9.10", "1.10.0"), "not a string compare");
        assert!(is_newer("1.10.0", "2.0.0"));
        assert!(is_newer("1.10.0", "1.10.1"));
    }

    #[test]
    fn is_newer_is_false_for_anything_malformed() {
        assert!(!is_newer("1.10.0", "v1.11.0"));
        assert!(!is_newer("1.10.0", "1.11"));
        assert!(!is_newer("1.10.0", "1.11.0-rc1"));
        assert!(!is_newer("garbage", "1.11.0"));
        assert!(!is_newer("", ""));
    }

    #[test]
    fn asset_name_follows_the_release_workflow() {
        assert_eq!(
            asset_name("ai-usagebar-tray", "x86_64"),
            "ai-usagebar-tray-windows-x86_64.exe"
        );
        assert!(["x86_64", "aarch64", "unknown"].contains(&current_arch()));
    }

    #[test]
    fn select_downloads_pairs_all_three_binaries_tray_first() {
        let downloads = select_downloads(&full_release(), "x86_64").unwrap();

        let binaries: Vec<&str> = downloads.iter().map(|d| d.binary).collect();
        assert_eq!(binaries, BINARIES.to_vec());
        for download in &downloads {
            assert_eq!(download.exe.name, asset_name(download.binary, "x86_64"));
            assert_eq!(
                download.sha256.name,
                format!("{}.sha256", download.exe.name)
            );
        }
    }

    #[test]
    fn select_downloads_requires_the_tray_pair() {
        let mut assets: Vec<Asset> = pair("ai-usagebar", 5_000_000).to_vec();
        assets.extend(pair("ai-usagebar-tui", 5_000_000));
        let error = select_downloads(&release_with(assets), "x86_64").unwrap_err();
        assert!(
            error.contains("ai-usagebar-tray-windows-x86_64.exe"),
            "{error}"
        );

        // Exe without its sidecar is just as missing.
        let [tray_exe, _] = pair("ai-usagebar-tray", 5_000_000);
        assert!(select_downloads(&release_with(vec![tray_exe]), "x86_64").is_err());

        // Wrong arch: nothing matches.
        assert!(select_downloads(&full_release(), "aarch64").is_err());
    }

    #[test]
    fn select_downloads_skips_an_incomplete_optional_pair() {
        let mut assets: Vec<Asset> = pair("ai-usagebar-tray", 5_000_000).to_vec();
        assets.extend(pair("ai-usagebar-tui", 5_000_000));
        let [cli_exe, _] = pair("ai-usagebar", 5_000_000);
        assets.push(cli_exe); // sidecar missing → skipped

        let downloads = select_downloads(&release_with(assets), "x86_64").unwrap();

        let binaries: Vec<&str> = downloads.iter().map(|d| d.binary).collect();
        assert_eq!(binaries, vec!["ai-usagebar-tray", "ai-usagebar-tui"]);
    }

    #[test]
    fn select_downloads_rejects_empty_and_oversized_exes() {
        let mut assets: Vec<Asset> = pair("ai-usagebar-tray", MAX_ASSET_BYTES + 1).to_vec();
        let error = select_downloads(&release_with(assets.clone()), "x86_64").unwrap_err();
        assert!(error.contains("over the"), "{error}");

        assets = pair("ai-usagebar-tray", 0).to_vec();
        let error = select_downloads(&release_with(assets), "x86_64").unwrap_err();
        assert!(error.contains("empty"), "{error}");

        // An optional exe with a bad size is a broken release, not a skip.
        let mut assets: Vec<Asset> = pair("ai-usagebar-tray", 5_000_000).to_vec();
        assets.extend(pair("ai-usagebar-tui", MAX_ASSET_BYTES + 1));
        assert!(select_downloads(&release_with(assets), "x86_64").is_err());

        // Exactly the limit is still allowed.
        let assets: Vec<Asset> = pair("ai-usagebar-tray", MAX_ASSET_BYTES).to_vec();
        assert!(select_downloads(&release_with(assets), "x86_64").is_ok());
    }

    #[test]
    fn parse_sha256_sidecar_accepts_both_formats_and_folds_case() {
        let hex = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

        assert_eq!(
            parse_sha256_sidecar(&format!("{hex}  ai-usagebar-tray-windows-x86_64.exe\n")).unwrap(),
            hex
        );
        assert_eq!(parse_sha256_sidecar(hex).unwrap(), hex);
        assert_eq!(parse_sha256_sidecar(&format!("  {hex}\r\n")).unwrap(), hex);
        assert_eq!(
            parse_sha256_sidecar(&hex.to_ascii_uppercase()).unwrap(),
            hex,
            "uppercase is folded"
        );
    }

    #[test]
    fn parse_sha256_sidecar_rejects_garbage() {
        assert!(parse_sha256_sidecar("").is_err());
        assert!(parse_sha256_sidecar("   \n").is_err());
        assert!(
            parse_sha256_sidecar("deadbeef  name.exe").is_err(),
            "too short"
        );
        assert!(
            parse_sha256_sidecar(&"zz".repeat(32)).is_err(),
            "right length, not hex"
        );
        assert!(
            parse_sha256_sidecar(
                "name.exe  e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            )
            .is_err(),
            "name first is not the format"
        );
    }

    #[test]
    fn verify_sha256_streams_the_file_and_detects_mismatch() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("blob.bin");
        let contents: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &contents).unwrap();
        let expected = hex_lower(&Sha256::digest(&contents));

        assert_eq!(verify_sha256(&path, &expected), Ok(()));
        assert_eq!(
            verify_sha256(&path, &expected.to_ascii_uppercase()),
            Ok(()),
            "case-insensitive"
        );

        let wrong = format!("{}{}", &expected[1..], "0");
        let error = verify_sha256(&path, &wrong).unwrap_err();
        assert!(error.contains("mismatch"), "{error}");

        assert!(verify_sha256(&dir.path().join("absent"), &expected).is_err());
    }

    #[test]
    fn stage_swap_replaces_the_exe_and_keeps_the_old_one() {
        let dir = TempDir::new().unwrap();
        let install = dir.path().join("install");
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        let name = "ai-usagebar-tray.exe";
        std::fs::write(install.join(name), b"v1").unwrap();
        let staged = staging.join(name);
        std::fs::write(&staged, b"v2").unwrap();

        let written = stage_swap(&install, &[(name.to_string(), staged.clone())]).unwrap();

        assert_eq!(written, vec![install.join(name)]);
        assert_eq!(std::fs::read(install.join(name)).unwrap(), b"v2");
        assert_eq!(
            std::fs::read(install.join("ai-usagebar-tray.exe.old")).unwrap(),
            b"v1"
        );
        assert!(!staged.exists(), "the staged file was moved, not copied");

        // A second update replaces the stale `.old` rather than failing on it.
        std::fs::write(&staged, b"v3").unwrap();
        stage_swap(&install, &[(name.to_string(), staged.clone())]).unwrap();
        assert_eq!(std::fs::read(install.join(name)).unwrap(), b"v3");
        assert_eq!(
            std::fs::read(install.join("ai-usagebar-tray.exe.old")).unwrap(),
            b"v2"
        );
    }

    #[test]
    fn stage_swap_installs_a_binary_that_was_not_there_before() {
        let dir = TempDir::new().unwrap();
        let install = dir.path().join("install");
        std::fs::create_dir_all(&install).unwrap();
        let staged = dir.path().join("ai-usagebar-tui.exe");
        std::fs::write(&staged, b"new").unwrap();

        stage_swap(&install, &[("ai-usagebar-tui.exe".to_string(), staged)]).unwrap();

        assert_eq!(
            std::fs::read(install.join("ai-usagebar-tui.exe")).unwrap(),
            b"new"
        );
        assert!(!install.join("ai-usagebar-tui.exe.old").exists());
    }

    #[test]
    fn stage_swap_with_a_missing_staged_file_touches_nothing() {
        let dir = TempDir::new().unwrap();
        let install = dir.path().join("install");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join("ai-usagebar-tray.exe"), b"v1").unwrap();
        std::fs::write(install.join("ai-usagebar.exe"), b"v1").unwrap();
        let tray_staged = dir.path().join("ai-usagebar-tray.exe");
        std::fs::write(&tray_staged, b"v2").unwrap();

        let error = stage_swap(
            &install,
            &[
                ("ai-usagebar-tray.exe".to_string(), tray_staged.clone()),
                (
                    "ai-usagebar.exe".to_string(),
                    dir.path().join("never-downloaded.exe"),
                ),
            ],
        )
        .unwrap_err();

        assert!(error.contains("does not exist"), "{error}");
        assert_eq!(
            std::fs::read(install.join("ai-usagebar-tray.exe")).unwrap(),
            b"v1"
        );
        assert_eq!(
            std::fs::read(install.join("ai-usagebar.exe")).unwrap(),
            b"v1"
        );
        assert!(!install.join("ai-usagebar-tray.exe.old").exists());
        assert!(
            tray_staged.exists(),
            "the good download is kept for a retry"
        );
    }

    #[test]
    fn stage_swap_rolls_back_entries_already_swapped_when_a_later_one_fails() {
        let dir = TempDir::new().unwrap();
        let install = dir.path().join("install");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join("ai-usagebar-tray.exe"), b"v1").unwrap();
        std::fs::write(install.join("ai-usagebar.exe"), b"v1").unwrap();
        // A stale `.old` that is a non-empty directory cannot be removed, so
        // the second entry fails after the first has already been swapped.
        let blocker = install.join("ai-usagebar.exe.old");
        std::fs::create_dir_all(&blocker).unwrap();
        std::fs::write(blocker.join("keep"), b"x").unwrap();
        let tray_staged = dir.path().join("ai-usagebar-tray.exe");
        let cli_staged = dir.path().join("ai-usagebar.exe");
        std::fs::write(&tray_staged, b"v2").unwrap();
        std::fs::write(&cli_staged, b"v2").unwrap();

        let result = stage_swap(
            &install,
            &[
                ("ai-usagebar-tray.exe".to_string(), tray_staged.clone()),
                ("ai-usagebar.exe".to_string(), cli_staged.clone()),
            ],
        );

        assert!(result.is_err());
        assert_eq!(
            std::fs::read(install.join("ai-usagebar-tray.exe")).unwrap(),
            b"v1",
            "the tray swap was undone"
        );
        assert!(!install.join("ai-usagebar-tray.exe.old").exists());
        assert_eq!(
            std::fs::read(&tray_staged).unwrap(),
            b"v2",
            "staged file restored"
        );
        assert_eq!(
            std::fs::read(install.join("ai-usagebar.exe")).unwrap(),
            b"v1"
        );
        assert_eq!(std::fs::read(&cli_staged).unwrap(), b"v2");
    }

    #[test]
    fn stage_swap_refuses_names_that_leave_the_install_dir() {
        let dir = TempDir::new().unwrap();
        let staged = dir.path().join("x.exe");
        std::fs::write(&staged, b"x").unwrap();

        for name in ["../x.exe", "sub/x.exe", "sub\\x.exe", "", ".."] {
            let error = stage_swap(dir.path(), &[(name.to_string(), staged.clone())]).unwrap_err();
            assert!(error.contains("refusing"), "{name:?}: {error}");
        }
    }

    #[test]
    fn sweep_old_removes_only_exe_old_files() {
        let dir = TempDir::new().unwrap();
        for name in [
            "ai-usagebar-tray.exe.old",
            "ai-usagebar.exe.old",
            "ai-usagebar-tray.exe",
            "notes.old",
            "config.toml",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("dir.exe.old")).unwrap();

        assert_eq!(sweep_old(dir.path()), 2);

        let mut remaining: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        remaining.sort();
        assert_eq!(
            remaining,
            vec![
                "ai-usagebar-tray.exe",
                "config.toml",
                "dir.exe.old",
                "notes.old"
            ]
        );

        assert_eq!(sweep_old(dir.path()), 0, "nothing left to sweep");
        assert_eq!(sweep_old(&dir.path().join("absent")), 0);
    }

    #[test]
    fn update_state_round_trips_and_a_corrupt_file_is_the_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("update.json");
        let state = UpdateState {
            last_check_ms: 1_757_246_400_000,
            snoozed_version: Some("1.11.0".to_string()),
        };

        state.save_at(&path).unwrap();

        assert_eq!(UpdateState::load_at(&path), state);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"snoozed_version\": \"1.11.0\""), "{text}");

        assert_eq!(
            UpdateState::load_at(&dir.path().join("absent.json")),
            UpdateState::default()
        );
        let corrupt = dir.path().join("corrupt.json");
        std::fs::write(&corrupt, "{\"last_check_ms\": ").unwrap();
        assert_eq!(UpdateState::load_at(&corrupt), UpdateState::default());

        // Missing fields default rather than failing the whole load.
        let partial = dir.path().join("partial.json");
        std::fs::write(&partial, "{\"last_check_ms\": 5}").unwrap();
        assert_eq!(
            UpdateState::load_at(&partial),
            UpdateState {
                last_check_ms: 5,
                snoozed_version: None,
            }
        );
    }

    #[test]
    fn staging_dir_is_per_version_under_the_cache_root() {
        let root = Path::new("cache");
        assert_eq!(
            staging_dir(root, "1.11.0"),
            Path::new("cache").join("updates").join("1.11.0")
        );
    }
}
