"""Portable packaging contracts; never read the user's applications or config."""

import importlib.util
from pathlib import Path
import plistlib
import tempfile
import unittest


spec = importlib.util.spec_from_file_location(
    "package_app", Path(__file__).with_name("package-app.py")
)
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class PackageAppTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bins = self.root / "release binaries"
        self.bins.mkdir()
        for name in package.BINARIES:
            (self.bins / name).write_bytes(name.encode())
        self.app = self.root / "Applications" / "AI Usage.app"

    def test_launchable_bundle_identifies_tray_and_preserves_sibling_tools(self):
        package.package_app(self.bins, self.app, "1.32.0")
        contents = self.app / "Contents"
        with (contents / "Info.plist").open("rb") as handle:
            info = plistlib.load(handle)
        self.assertEqual(info["CFBundleIdentifier"], "com.akitaonrails.ai-usagebar-tray")
        self.assertEqual(info["CFBundlePackageType"], "APPL")
        self.assertTrue(info["LSUIElement"])
        self.assertEqual(info["CFBundleShortVersionString"], "1.32.0")
        self.assertEqual(info["CFBundleVersion"], "1.32.0")
        self.assertEqual(info["CFBundleExecutable"], "ai-usagebar-tray")
        icon = contents / "Resources" / (info["CFBundleIconFile"] + ".icns")
        self.assertEqual(icon.read_bytes()[:4], b"icns")
        for name in ("menu-bar-icon.LICENSE", "LICENSE-APACHE-2.0"):
            self.assertEqual(
                (contents / "Resources" / name).read_bytes(),
                Path(__file__).with_name(name).read_bytes(),
            )
        for name in package.BINARIES:
            binary = contents / "MacOS" / name
            self.assertEqual(binary.read_bytes(), (self.bins / name).read_bytes())
            self.assertEqual(binary.stat().st_mode & 0o777, 0o755)
            self.assertFalse(binary.is_symlink())

    def test_missing_binary_does_not_leave_a_partial_application(self):
        (self.bins / "ai-usagebar-tui").unlink()
        with self.assertRaises(FileNotFoundError):
            package.package_app(self.bins, self.app, "1.32.0")
        self.assertFalse(self.app.exists())

    def test_existing_application_is_not_overwritten(self):
        self.app.mkdir(parents=True)
        marker = self.app / "keep"
        marker.write_text("existing app")
        with self.assertRaises(FileExistsError):
            package.package_app(self.bins, self.app, "1.32.0")
        self.assertEqual(marker.read_text(), "existing app")

    def test_requires_an_application_bundle_suffix(self):
        with self.assertRaises(ValueError):
            package.package_app(self.bins, self.root / "bare-directory", "1.32.0")


if __name__ == "__main__":
    unittest.main()
