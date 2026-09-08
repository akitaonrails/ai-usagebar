//! Build the Windows tray WebView (Vite) before compiling `src/tray/host.rs`.

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
    if os != "windows" {
        return;
    }

    ensure_deps(&popover);
    npm(&popover, &["run", "build"]);
    assert_dist(&popover);
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
