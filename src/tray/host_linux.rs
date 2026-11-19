//! Linux system-tray host placeholder. The next slice implements the event
//! loop, KSNI icon, WebView and IPC here; this build only reports that the
//! host is not available yet.

pub fn run() -> i32 {
    eprintln!("ai-usagebar-tray: the Linux host is not implemented yet.");
    1
}
