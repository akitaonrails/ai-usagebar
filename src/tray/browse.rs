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

#[cfg(any(windows, target_os = "macos"))]
pub fn open(raw: &str) {
    let Some(url) = http_url(raw) else {
        return;
    };
    let _ = command(url).spawn();
}

/// The process that opens `url` in the default browser, with the URL as its
/// only argument. On Windows that is Explorer, as for the device-code sign-in
/// page (`nous::oauth::SystemBrowserOpener`): `cmd /C start` read the URL's
/// `&`, `|`, `^` and `%NAME%` as cmd syntax rather than as part of the link.
#[cfg(any(windows, target_os = "macos"))]
fn command(url: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        let mut command = std::process::Command::new("explorer.exe");
        command.arg(url);
        command
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg(url);
        command
    }
}

#[cfg(test)]
mod tests {
    use super::http_url;

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

    /// A query string's `&`, `%` and `^` stay inside the opener's one
    /// argument: nothing parses the URL as shell syntax.
    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn the_url_is_the_openers_only_argument() {
        let url = "https://example.com/usage?a=1&b=%PATH%^";
        let command = super::command(url);
        let program = if cfg!(windows) {
            "explorer.exe"
        } else {
            "/usr/bin/open"
        };
        assert_eq!(command.get_program(), program);
        assert_eq!(command.get_args().collect::<Vec<_>>(), [url]);
    }
}
