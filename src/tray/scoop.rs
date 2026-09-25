//! Scoop ownership and hand-off for a Windows tray update.
//!
//! The decisions in this module are path and string operations so they can be
//! tested on every platform. Only the final PowerShell process is Windows
//! specific; the host supplies the detached process policy.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine;

use super::RELAUNCH_ENV;

/// A tray executable managed by Scoop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoopApp {
    pub name: String,
    pub root: PathBuf,
}

/// Detect the running executable in Scoop's `<root>/apps/<name>/<version>/`
/// tree. `scoop_managed` is the shared check for Scoop's `install.json`; this
/// module adds the app-name and root-shape checks needed before building a
/// PowerShell command.
pub fn detect_with(exe: &Path, exists: impl Fn(&Path) -> bool) -> Option<ScoopApp> {
    if !crate::update::scoop_managed(exe, &exists) {
        return None;
    }
    let version_dir = exe.parent()?;
    let app_dir = version_dir.parent()?;
    let apps_dir = app_dir.parent()?;
    if !apps_dir
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("apps"))
    {
        return None;
    }
    let root = apps_dir.parent()?.to_path_buf();
    let name = app_dir.file_name()?.to_str()?;
    is_plain_name(name).then(|| ScoopApp {
        name: name.to_owned(),
        root,
    })
}

/// Detect the current executable as a Scoop install.
pub fn detect() -> Option<ScoopApp> {
    let exe = std::env::current_exe().ok()?;
    detect_with(&exe, Path::is_file)
}

fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

impl ScoopApp {
    /// Whether the Scoop PowerShell shim is available beneath this install's
    /// root. A global Scoop install normally has the same shim under
    /// ProgramData.
    pub fn can_run(&self, exists: impl Fn(&Path) -> bool) -> bool {
        exists(&self.root.join("shims").join("scoop.ps1"))
    }
}

/// Quote text as a PowerShell single-quoted literal body. PowerShell accepts
/// doubled quote characters for apostrophes; double all apostrophe variants so
/// paths remain unambiguous even when they contain typographic punctuation.
fn ps_quote(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => {
                format!("{ch}{ch}")
            }
            _ => ch.to_string(),
        })
        .collect()
}

/// Build the detached update script. All paths and the Scoop app name are
/// inserted only as single-quoted PowerShell literals.
///
/// Success is Scoop's `current\manifest.json` reaching `version`, not an exit code: `scoop
/// update <app>` exits 0 when its bucket has nothing newer yet. Each Scoop command runs in a
/// child PowerShell (`-File`), so nothing Scoop does to its session (an `exit`, a preference, a
/// strict mode) can stop this script before it relaunches the tray.
pub fn update_script(app: &ScoopApp, tray_pid: u32, log: &Path, version: &str) -> String {
    let wanted = ps_quote(version);
    let log_text = ps_quote(&log.display().to_string());
    let log_dir = ps_quote(
        &log.parent()
            .unwrap_or_else(|| Path::new("."))
            .display()
            .to_string(),
    );
    let root = &app.root;
    let scoop = ps_quote(&root.join("shims").join("scoop.ps1").display().to_string());
    let name = ps_quote(&app.name);
    let manifest = ps_quote(
        &root
            .join("apps")
            .join(&app.name)
            .join("current")
            .join("manifest.json")
            .display()
            .to_string(),
    );
    let tray = ps_quote(
        &root
            .join("apps")
            .join(&app.name)
            .join("current")
            .join("ai-usagebar-tray.exe")
            .display()
            .to_string(),
    );

    format!(
        "New-Item -ItemType Directory -Force -Path '{log_dir}' | Out-Null\n\
Start-Transcript -Path '{log_text}' -Append\n\
Write-Output ('PowerShell ' + $PSVersionTable.PSVersion + '; git: ' + (& git --version 2>$null))\n\
Wait-Process -Id {tray_pid} -ErrorAction SilentlyContinue\n\
$ps = (Get-Process -Id $PID).Path\n\
$installed = $null\n\
for ($attempt = 1; $attempt -le 3; $attempt++) {{\n\
    & $ps -NoProfile -NonInteractive -ExecutionPolicy Bypass -File '{scoop}' update 2>&1 | Out-Host\n\
    & $ps -NoProfile -NonInteractive -ExecutionPolicy Bypass -File '{scoop}' update '{name}' 2>&1 | Out-Host\n\
    $installed = $null\n\
    try {{ $installed = (Get-Content '{manifest}' -ErrorAction Stop | ConvertFrom-Json).version }} catch {{}}\n\
    Write-Output ('Attempt ' + $attempt + ': Scoop exited ' + $LASTEXITCODE + ', current is ' + $installed + ', wanted {wanted}')\n\
    if ($installed -eq '{wanted}') {{ break }}\n\
    if ($attempt -lt 3) {{ Start-Sleep -Seconds 5 }}\n\
}}\n\
$env:{RELAUNCH_ENV} = '1'\n\
Start-Process -FilePath '{tray}'\n\
Stop-Transcript"
    )
}

/// Encode a PowerShell script as the UTF-16LE payload expected by
/// `-EncodedCommand`.
pub fn encoded_command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Build the hidden Windows PowerShell 5.1 process used for the hand-off.
pub fn powershell_command(system_root: &Path, script: &str) -> Command {
    let program = system_root
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let mut command = Command::new(program);
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-EncodedCommand",
        ])
        .arg(encoded_command(script))
        .env_remove("PSModulePath")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(crate::process::CREATE_NO_WINDOW);
    }
    command
}

/// `CREATE_BREAKAWAY_FROM_JOB` from `PROCESS_CREATION_FLAGS`, spelled out like
/// [`crate::process::CREATE_NO_WINDOW`].
#[cfg(windows)]
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// Spawn Scoop's detached hand-off for `version` and return without waiting for it.
///
/// A tray started through Scoop's shim (`ai-usagebar-tray` typed in a terminal) runs inside the
/// shim's job object, and a job can kill every process in it once the shim exits, which it does
/// when the tray quits for the update. So the hand-off first asks to leave the job; a job that
/// does not allow that makes the spawn fail, and it is started inside the job instead.
pub fn spawn_update(
    app: &ScoopApp,
    tray_pid: u32,
    log: &Path,
    version: &str,
) -> Result<(), String> {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let mut command = powershell_command(&system_root, &update_script(app, tray_pid, log, version));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(crate::process::CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
        if command.spawn().is_ok() {
            return Ok(());
        }
        command.creation_flags(crate::process::CREATE_NO_WINDOW);
    }
    command.spawn().map(drop).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::fs;
    use tempfile::tempdir;

    fn decode_command(encoded: &str) -> String {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("valid base64");
        let (pairs, rest) = bytes.as_chunks::<2>();
        assert!(rest.is_empty(), "UTF-16LE has an even byte count");
        let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        String::from_utf16(&units).expect("valid UTF-16LE")
    }

    #[test]
    fn detects_literal_current_and_version_paths() {
        for suffix in ["current", "1.24.0"] {
            let exe = PathBuf::from(format!(
                "C:/Users/me/scoop/apps/ai-usagebar/{suffix}/ai-usagebar-tray.exe"
            ));
            let app =
                detect_with(&exe, |path| path.ends_with("install.json")).expect("Scoop layout");
            assert_eq!(app.name, "ai-usagebar");
            assert_eq!(app.root, Path::new("C:/Users/me/scoop"));
        }
    }

    #[test]
    fn detects_tempdir_tree_and_rejects_invalid_layouts() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let app_dir = root.join("apps").join("ai-usagebar");
        let version_dir = app_dir.join("current");
        fs::create_dir_all(&version_dir).unwrap();
        fs::write(version_dir.join("install.json"), b"{}").unwrap();
        let exe = version_dir.join("ai-usagebar-tray.exe");
        fs::write(&exe, b"").unwrap();
        assert_eq!(
            detect_with(&exe, Path::is_file),
            Some(ScoopApp {
                name: "ai-usagebar".into(),
                root: root.into(),
            })
        );

        fs::remove_file(version_dir.join("install.json")).unwrap();
        assert_eq!(detect_with(&exe, Path::is_file), None);

        let no_apps = root
            .join("without-apps")
            .join("ai-usagebar")
            .join("current");
        fs::create_dir_all(&no_apps).unwrap();
        fs::write(no_apps.join("install.json"), b"{}").unwrap();
        let no_apps_exe = no_apps.join("ai-usagebar-tray.exe");
        assert_eq!(detect_with(&no_apps_exe, Path::is_file), None);

        let unsafe_dir = root.join("apps").join("a$(b)").join("current");
        fs::create_dir_all(&unsafe_dir).unwrap();
        fs::write(unsafe_dir.join("install.json"), b"{}").unwrap();
        let unsafe_exe = unsafe_dir.join("ai-usagebar-tray.exe");
        assert_eq!(detect_with(&unsafe_exe, Path::is_file), None);

        let fork_dir = root.join("apps").join("ai-usagebar-dev").join("current");
        fs::create_dir_all(&fork_dir).unwrap();
        fs::write(fork_dir.join("install.json"), b"{}").unwrap();
        let fork_exe = fork_dir.join("ai-usagebar-tray.exe");
        assert_eq!(
            detect_with(&fork_exe, Path::is_file).map(|app| app.name),
            Some("ai-usagebar-dev".into())
        );
    }

    #[test]
    fn can_run_requires_the_scoop_shim() {
        let app = ScoopApp {
            name: "ai-usagebar".into(),
            root: PathBuf::from("C:/ProgramData/scoop"),
        };
        assert!(app.can_run(|path| path == app.root.join("shims").join("scoop.ps1")));
        assert!(!app.can_run(|_| false));
    }

    #[test]
    fn ps_quote_doubles_ascii_and_typographic_apostrophes() {
        assert_eq!(ps_quote("C:\\Users\\O'Brien"), "C:\\Users\\O''Brien");
        assert_eq!(
            ps_quote("O\u{2018}Brien O\u{2019}Brien O\u{201a}Brien O\u{201b}Brien"),
            "O\u{2018}\u{2018}Brien O\u{2019}\u{2019}Brien O\u{201a}\u{201a}Brien O\u{201b}\u{201b}Brien"
        );
    }

    #[test]
    fn encoded_script_contains_safe_update_order_and_relaunch() {
        let app = ScoopApp {
            name: "ai-usagebar".into(),
            root: PathBuf::from("C:/Users/O'Brien/scoop"),
        };
        let script = update_script(
            &app,
            4242,
            Path::new("C:/Users/O'Brien/AppData/Local/ai-usagebar/updates/scoop.log"),
            "1.25.0",
        );
        let decoded = decode_command(&encoded_command(&script));
        assert!(decoded.contains("Wait-Process -Id 4242"), "{decoded}");
        // Scoop runs in a child PowerShell, so nothing it does to its session ends this script.
        assert!(
            decoded.contains("& $ps -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "),
            "{decoded}"
        );
        // Success is the installed manifest reaching the wanted version, not an exit code.
        assert!(
            decoded.contains("if ($installed -eq '1.25.0') { break }"),
            "{decoded}"
        );
        assert!(!decoded.contains("$LASTEXITCODE -eq"), "{decoded}");
        let bucket = decoded.find("update 2>&1").expect("bucket update");
        let app_update = decoded.find("update 'ai-usagebar'").expect("app update");
        assert!(bucket < app_update, "{decoded}");
        assert!(decoded.contains("$attempt -le 3"), "{decoded}");
        assert!(
            decoded.contains("update 'ai-usagebar' 2>&1 | Out-Host"),
            "{decoded}"
        );
        assert!(decoded.contains("Start-Sleep -Seconds 5"), "{decoded}");
        // Built with the same joins as the script, so the separators match on every OS.
        let current_exe = app
            .root
            .join("apps")
            .join("ai-usagebar")
            .join("current")
            .join("ai-usagebar-tray.exe")
            .display()
            .to_string()
            .replace('\'', "''");
        assert!(decoded.contains(&current_exe), "{decoded}");
        assert!(
            decoded.contains("$env:AIUB_TRAY_RELAUNCH = '1'"),
            "{decoded}"
        );
        assert!(!decoded.contains('"'), "{decoded}");
    }

    #[test]
    fn powershell_command_uses_injected_root_and_scrubs_module_path() {
        let command = powershell_command(Path::new("C:/Windows"), "Write-Output 'ok'");
        assert_eq!(
            command.get_program(),
            Path::new("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe")
        );
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args[..7],
            [
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
                "-EncodedCommand",
            ]
        );
        assert_eq!(decode_command(&args[7]), "Write-Output 'ok'");
        let removed: Vec<_> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(removed, vec!["PSModulePath"]);
    }
}
