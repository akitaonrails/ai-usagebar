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
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
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
}
