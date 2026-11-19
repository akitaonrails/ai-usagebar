#!/usr/bin/env python3
"""Wrap the release executables in a macOS application bundle.

Usage: python3 macos/package-app.py BIN_DIR OUTPUT.app VERSION
The standalone release assets remain available for CLI users and updates.
"""

import argparse
from pathlib import Path
import plistlib
import shutil


BINARIES = ("ai-usagebar-tray", "ai-usagebar", "ai-usagebar-tui")
BUNDLE_ID = "com.akitaonrails.ai-usagebar-tray"


def package_app(bin_dir: Path, output: Path, version: str) -> None:
    if output.suffix != ".app":
        raise ValueError("output must end in .app")
    if output.exists():
        raise FileExistsError(f"refusing to overwrite {output}")
    # Validate before creating a partial bundle. Keep the CLI and TUI next to
    # the tray: Open TUI and the self-updater resolve these sibling binaries.
    for name in BINARIES:
        if not (bin_dir / name).is_file():
            raise FileNotFoundError(bin_dir / name)
    contents = output / "Contents"
    executables = contents / "MacOS"
    executables.mkdir(parents=True)
    resources = contents / "Resources"
    resources.mkdir()
    shutil.copy2(Path(__file__).with_name("AIUsage.icns"), resources / "AIUsage.icns")
    for name in ("menu-bar-icon.LICENSE", "LICENSE-APACHE-2.0"):
        shutil.copy2(Path(__file__).with_name(name), resources / name)
    for name in BINARIES:
        destination = executables / name
        shutil.copy2(bin_dir / name, destination)
        destination.chmod(0o755)
    with (contents / "Info.plist").open("wb") as handle:
        plistlib.dump(
            {
                "CFBundleIdentifier": BUNDLE_ID,
                "CFBundleName": "AI Usage",
                "CFBundleDisplayName": "AI Usage",
                "CFBundleExecutable": "ai-usagebar-tray",
                "CFBundlePackageType": "APPL",
                "CFBundleInfoDictionaryVersion": "6.0",
                "CFBundleIconFile": "AIUsage",
                "CFBundleShortVersionString": version,
                "CFBundleVersion": version,
                "LSUIElement": True,
                "NSHighResolutionCapable": True,
            },
            handle,
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bin_dir", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("version")
    args = parser.parse_args()
    package_app(args.bin_dir, args.output, args.version)
