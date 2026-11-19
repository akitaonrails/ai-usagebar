//! Open a provider Status/Dashboard URL in the default browser.

/// Accept only a single http(s) URL. Rejects whitespace, control chars, and
/// anything that is not a web URL so the host never shells out on IPC junk.
pub fn http_url(raw: &str) -> Option<&str> {
    let url = raw.trim();
    if url.len() < 8 || url.len() > 2048 {
        return None;
    }
    if url.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return None;
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        Some(url)
    } else {
        None
    }
}

// On Linux the host lands behind an opt-in feature; until it calls `open`
// the only user is the future host, so silence its dead-code warning.
#[cfg_attr(all(target_os = "linux", feature = "linux-tray"), allow(dead_code))]
#[cfg(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", feature = "linux-tray")
))]
pub fn open(raw: &str) {
    let Some(url) = http_url(raw) else {
        return;
    };
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/usr/bin/open").arg(url).spawn();
    }
    #[cfg(all(target_os = "linux", feature = "linux-tray"))]
    {
        // PATH lookup is deliberate: NixOS has no /usr/bin. `arg` passes the
        // URL as one argument; no shell ever interprets it.
        let _ = spawn_reaped(std::process::Command::new("xdg-open").arg(url));
    }
}

/// Spawns `command` and collects its exit status on a separate thread, so a
/// finished `xdg-open` never lingers as a zombie for the life of the tray and
/// the calling (GTK) thread never blocks. Returns the child's pid and the
/// reaper's handle.
#[cfg(all(target_os = "linux", feature = "linux-tray"))]
fn spawn_reaped(
    command: &mut std::process::Command,
) -> std::io::Result<(u32, std::thread::JoinHandle<()>)> {
    let mut child = command.spawn()?;
    let pid = child.id();
    // `Builder::spawn` reports a refused thread instead of panicking on the
    // GTK thread. Without a reaper the child is left to finish on its own and
    // lingers as a zombie, which is the lesser failure.
    let reaper = std::thread::Builder::new()
        .name("xdg-open-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        })?;
    Ok((pid, reaper))
}

#[cfg(test)]
mod tests {
    use super::http_url;

    #[cfg(all(target_os = "linux", feature = "linux-tray"))]
    #[test]
    fn a_finished_child_is_collected_not_left_as_a_zombie() {
        let (pid, reaper) =
            super::spawn_reaped(std::process::Command::new("true").arg("ignored")).unwrap();
        reaper.join().unwrap();
        // A reaped pid has no /proc entry; an uncollected child stays as `Z`.
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "pid {pid} was not collected"
        );
    }

    #[test]
    fn accepts_http_and_https() {
        assert_eq!(
            http_url("https://status.anthropic.com/"),
            Some("https://status.anthropic.com/")
        );
        assert_eq!(
            http_url("  http://status.openai.com  "),
            Some("http://status.openai.com")
        );
    }

    #[test]
    fn rejects_non_web_and_junk() {
        assert_eq!(http_url("javascript:alert(1)"), None);
        assert_eq!(http_url("file:///etc/passwd"), None);
        assert_eq!(http_url("https://ok.example\nhttps://evil"), None);
        assert_eq!(http_url(""), None);
    }
}
