//! Which origin the tray's IPC bridge and the popover's WebView accept.
//!
//! The bridge answers `window.ipc.postMessage` and now writes to `config.toml`
//! (`set-api-key`), so only the popover's own frame may reach it; and the
//! WebView must not navigate outside that frame. Both hosts load the popover
//! from two different origins — macOS and the opt-in Linux host from
//! `aiub://localhost/index.html`, Windows from
//! `http://aiub.localhost/index.html` — and wry hands each handler the URI to
//! check:
//!
//! - on macOS and Windows, the IPC handler receives a `Request` whose URI is the sending frame's
//!   full absolute URL: `frameInfo.request().URL().absoluteString()` on macOS
//!   (wry 0.57.0, `src/wkwebview/class/wry_web_view_delegate.rs:50-53,60-61`)
//!   and `ICoreWebView2WebMessageReceivedEventArgs::Source`, "the source URL
//!   of the script that sent the message", on Windows
//!   (wry 0.57.0, `src/webview2/mod.rs:964-965,977-978`);
//! - the navigation handler receives the URL the WebView is about to load
//!   (`NavigationStarting` on Windows, `webview2/mod.rs:748-754`; the
//!   navigation policy on macOS, `wry_navigation_delegate.rs:120-124`);
//! - the new-window handler receives the URL `window.open` asked for
//!   (`NewWindowRequested` on Windows, `webview2/mod.rs:777-781`;
//!   `createWebViewWithConfiguration` on macOS,
//!   `wry_web_view_ui_delegate.rs:180-198`).
//!
//! These are pure decisions over those URI strings, tested on every OS;
//! `host_macos.rs`, `host.rs` and the opt-in `host_linux.rs` apply them in
//! their WebView builders.
//! WebKitGTK reports the top-level URI for IPC. The Linux host uses the
//! exact embedded page and a frame-denying CSP instead of treating that
//! URI as the sender's origin.

/// The OS the running host is on. Each host constructs only its own variant,
/// so the others are dead on that platform (Linux's variant only exists with
/// the feature that builds its host).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Os {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Macos,
    #[cfg_attr(not(windows), allow(dead_code))]
    Windows,
    #[cfg_attr(
        not(all(target_os = "linux", feature = "linux-tray")),
        allow(dead_code)
    )]
    Linux,
}

/// The scheme and host the popover loads from on `os`, exactly as the hosts'
/// `build_webview` loads them. Linux shares macOS's origin on purpose: both
/// hosts load `aiub://localhost/index.html`.
fn expected_origin(os: Os) -> (&'static str, &'static str) {
    match os {
        Os::Macos | Os::Linux => ("aiub", "localhost"),
        Os::Windows => ("http", "aiub.localhost"),
    }
}

/// Whether `uri` comes from the popover's own origin on `os`: the scheme and
/// the host, compared exactly. The host is the authority up to the first `/`,
/// `?` or `#`, after any `user:pass@` userinfo. Not the origin: a suffix such
/// as `localhost.evil`, another scheme, an explicit port, an empty host, a
/// missing scheme, or a missing `//` authority.
pub(crate) fn is_popover_origin(uri: &str, os: Os) -> bool {
    match scheme_and_host(uri) {
        Some((scheme, host)) => {
            let (expected_scheme, expected_host) = expected_origin(os);
            scheme.eq_ignore_ascii_case(expected_scheme) && host.eq_ignore_ascii_case(expected_host)
        }
        None => false,
    }
}

/// What a navigation or a `window.open` to `url` may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Navigation {
    /// The URL is the popover's own origin: the WebView may load it.
    Allow,
    /// Another web URL: the host hands it to `browse::open` (the `open-url`
    /// command's path) and denies it in the WebView.
    OpenExternal,
    /// Neither the popover's origin nor a web URL: deny, open nothing.
    Deny,
}

/// The decision for a navigation or new-window request to `url` on `os`.
///
/// Only the popover's own origin loads in the WebView. Every other URL that
/// `browse::http_url` — the same gate the `open-url` command uses — accepts
/// goes to the default browser; anything else (`file://`, `javascript:`,
/// another scheme, no scheme, or junk the gate rejects) is denied without
/// opening.
pub(crate) fn navigation(url: &str, os: Os) -> Navigation {
    if is_popover_origin(url, os) {
        return Navigation::Allow;
    }
    if super::browse::http_url(url).is_some() {
        Navigation::OpenExternal
    } else {
        Navigation::Deny
    }
}

/// The lowercased `(scheme, host)` of `uri`, when it has a scheme with the
/// WHATWG grammar (a letter, then letters/digits/`+`/`-`/`.`) and a `//`
/// authority whose host is non-empty and carries no explicit port; `None`
/// otherwise.
fn scheme_and_host(uri: &str) -> Option<(String, String)> {
    let (scheme, rest) = uri.split_once(':')?;
    if !valid_scheme(scheme) {
        return None;
    }
    let rest = rest.strip_prefix("//")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host.is_empty() || host.contains(':') {
        return None;
    }
    Some((scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

fn valid_scheme(scheme: &str) -> bool {
    let mut chars = scheme.bytes();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {
            chars.all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.')
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_popover_origin_of_its_own_os() {
        assert!(is_popover_origin("aiub://localhost/index.html", Os::Macos));
        // Linux's host loads the same page from the same custom scheme.
        assert!(is_popover_origin("aiub://localhost/index.html", Os::Linux));
        assert!(is_popover_origin(
            "http://aiub.localhost/index.html",
            Os::Windows
        ));
        // The bare origin and other paths of it are the origin too.
        assert!(is_popover_origin("aiub://localhost/", Os::Macos));
        assert!(is_popover_origin("aiub://localhost/", Os::Linux));
        assert!(is_popover_origin("http://aiub.localhost", Os::Windows));
    }

    #[test]
    fn rejects_other_hosts_and_schemes() {
        // A host that merely starts with the origin's host is not the origin.
        assert!(!is_popover_origin("aiub://localhost.evil/", Os::Macos));
        assert!(!is_popover_origin("aiub://localhost.evil/", Os::Linux));
        assert!(!is_popover_origin("aiub://evil/", Os::Macos));
        assert!(!is_popover_origin(
            "http://aiub.localhost.evil.com/",
            Os::Windows
        ));
        // A different scheme is a different origin, even for the same host.
        assert!(!is_popover_origin("https://aiub.localhost/", Os::Windows));
        assert!(!is_popover_origin("http://aiub.localhost/", Os::Macos));
        assert!(!is_popover_origin("http://aiub.localhost/", Os::Linux));
        // Each OS accepts only its own origin; macOS and Linux share one.
        assert!(!is_popover_origin(
            "aiub://localhost/index.html",
            Os::Windows
        ));
        assert!(!is_popover_origin(
            "http://aiub.localhost/index.html",
            Os::Macos
        ));
        assert!(!is_popover_origin(
            "http://aiub.localhost/index.html",
            Os::Linux
        ));
    }

    #[test]
    fn rejects_malformed_uris() {
        assert!(!is_popover_origin("", Os::Macos));
        assert!(!is_popover_origin("null", Os::Macos));
        // No scheme, a `//` without one, or a scheme without the `//`
        // authority.
        assert!(!is_popover_origin("localhost/index.html", Os::Macos));
        assert!(!is_popover_origin("//localhost/index.html", Os::Macos));
        assert!(!is_popover_origin("aiub:localhost/index.html", Os::Macos));
        assert!(!is_popover_origin("javascript:alert(1)", Os::Macos));
        assert!(!is_popover_origin("file:///etc/passwd", Os::Macos));
        assert!(!is_popover_origin("", Os::Linux));
        assert!(!is_popover_origin("aiub:localhost/", Os::Linux));
    }

    #[test]
    fn a_port_is_a_different_origin_and_a_userinfo_hides_the_host() {
        // The popover loads without a port; one makes a different origin.
        assert!(!is_popover_origin(
            "aiub://localhost:8080/index.html",
            Os::Macos
        ));
        assert!(!is_popover_origin(
            "aiub://localhost:8080/index.html",
            Os::Linux
        ));
        assert!(!is_popover_origin("http://aiub.localhost:80/", Os::Windows));
        // In `localhost@evil` the `localhost` is a userinfo; the host is
        // `evil`. A userinfo on the right host is still the origin.
        assert!(!is_popover_origin("aiub://localhost@evil/", Os::Macos));
        assert!(!is_popover_origin("aiub://localhost@evil/", Os::Linux));
        assert!(is_popover_origin(
            "http://user:pass@aiub.localhost/",
            Os::Windows
        ));
        assert!(is_popover_origin("aiub://user:pass@localhost/", Os::Linux));
    }

    #[test]
    fn navigation_allows_only_the_popover_origin() {
        assert_eq!(
            navigation("aiub://localhost/index.html", Os::Macos),
            Navigation::Allow
        );
        assert_eq!(
            navigation("http://aiub.localhost/index.html", Os::Windows),
            Navigation::Allow
        );
        assert_eq!(
            navigation("aiub://localhost/index.html", Os::Linux),
            Navigation::Allow
        );
        // The other OS's origin is not this origin.
        assert_ne!(
            navigation("aiub://localhost/index.html", Os::Windows),
            Navigation::Allow
        );
        assert_eq!(
            navigation("http://aiub.localhost/index.html", Os::Linux),
            Navigation::OpenExternal
        );
    }

    #[test]
    fn navigation_sends_other_web_urls_outside_and_denies_them() {
        assert_eq!(
            navigation("https://example.com", Os::Macos),
            Navigation::OpenExternal
        );
        assert_eq!(
            navigation("http://example.com", Os::Windows),
            Navigation::OpenExternal
        );
        // Junk the `open-url` gate (`browse::http_url`) rejects is denied,
        // not opened.
        assert_eq!(
            navigation("https://ok.example\nhttps://evil", Os::Macos),
            Navigation::Deny
        );
        // The gate is case-sensitive on the prefix; uppercase is not opened.
        assert_eq!(
            navigation("HTTPS://EXAMPLE.COM", Os::Windows),
            Navigation::Deny
        );
    }

    #[test]
    fn navigation_denies_non_web_schemes_without_opening() {
        assert_eq!(
            navigation("file:///etc/passwd", Os::Macos),
            Navigation::Deny
        );
        assert_eq!(
            navigation("javascript:alert(1)", Os::Windows),
            Navigation::Deny
        );
        assert_eq!(
            navigation("data:text/html,alert(1)", Os::Macos),
            Navigation::Deny
        );
        assert_eq!(navigation("", Os::Windows), Navigation::Deny);
        assert_eq!(navigation("null", Os::Macos), Navigation::Deny);
    }
}
