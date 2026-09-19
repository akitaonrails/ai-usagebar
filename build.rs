//! Build the tray WebView (Vite) before compiling the Windows/macOS host.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let popover = manifest.join("windows").join("popover");
    println!("cargo:rerun-if-changed={}", popover.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        popover.join("index.html").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        popover.join("package.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        popover.join("package-lock.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        popover.join("vite.config.ts").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        popover.join("components.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        popover.join("dist").join("index.html").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        popover.join("dist").join("popover.js").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        popover.join("dist").join("popover.css").display()
    );

    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os != "windows" && os != "macos" {
        return;
    }

    if npm_available() {
        ensure_deps(&popover);
        npm(&popover, &["run", "build"]);
        assert_dist(&popover);
    } else if !dist_complete(&popover) {
        write_stub_dist(&popover);
    }
}

fn npm_available() -> bool {
    Command::new("npm")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn dist_complete(dir: &Path) -> bool {
    ["index.html", "popover.js", "popover.css"]
        .iter()
        .all(|name| dir.join("dist").join(name).is_file())
}

fn write_stub_dist(dir: &Path) {
    let dist = dir.join("dist");
    let _ = std::fs::create_dir_all(&dist);
    let _ = std::fs::write(
        dist.join("index.html"),
        "<!doctype html><title>ai-usagebar</title><p>popover dist missing</p>\n",
    );
    let _ = std::fs::write(dist.join("popover.js"), "/* stub */\n");
    let _ = std::fs::write(dist.join("popover.css"), "/* stub */\n");
}

fn ensure_deps(dir: &Path) {
    if dir.join("node_modules").join("vite").exists() {
        return;
    }
    if dir.join("package-lock.json").exists() {
        npm(dir, &["ci"]);
    } else {
        npm(dir, &["install"]);
    }
}

fn assert_dist(dir: &Path) {
    for name in ["index.html", "popover.js", "popover.css"] {
        let path: PathBuf = dir.join("dist").join(name);
        if !path.is_file() {
            panic!(
                "Windows tray UI build did not emit {} — `npm run build` in {}",
                path.display(),
                dir.display()
            );
        }
    }
}

fn npm(dir: &Path, args: &[&str]) {
    let mut command = if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "npm"]);
        cmd
    } else {
        Command::new("npm")
    };
    let status = command
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|error| panic!("npm {} failed to start: {error}", args.join(" ")));
    if !status.success() {
        panic!(
            "npm {} failed in {} (status {status}). Install Node.js 20+, then retry.",
            args.join(" "),
            dir.display()
        );
    }
}
