"""GTK and local-socket regressions; run with make mint-runtime-test."""

import importlib.machinery
import importlib.util
import json
import os
import socket
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


SOURCE = Path(__file__).with_name("ai-usagebar-tray")
LOADER = importlib.machinery.SourceFileLoader("mint_tray_runtime", str(SOURCE))
SPEC = importlib.util.spec_from_loader(LOADER.name, LOADER)
tray = importlib.util.module_from_spec(SPEC)
LOADER.exec_module(tray)


class FakeWindow:
    def __init__(self):
        self.visible = True
        self.active = False
        self.focused_since_open = False
        self.focus_started_at = 0.0
        self.focus_retry_pending = False
        self.presented = []

    def hide(self):
        self.visible = False

    def is_visible(self):
        return self.visible

    def is_active(self):
        return self.active

    def present(self):
        self.presented.append("retry")

    def show_all(self):
        self.visible = True

    def get_window(self):
        return object()

    def present_with_time(self, timestamp):
        self.presented.append(timestamp)

    def retry_focus(self):
        return tray.UsageDashboard.retry_focus(self)


class TrayRuntimeTest(unittest.TestCase):
    def test_installer_persists_custom_binary_and_cargo_tui_launcher(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            custom = home / "custom location/ai-usagebar"
            tui = home / ".cargo/bin/ai-usagebar-tui"
            for executable in (custom, tui):
                executable.parent.mkdir(parents=True, exist_ok=True)
                executable.write_text("#!/bin/sh\n")
                executable.chmod(0o755)
            env = {**os.environ, "HOME": directory, "AI_USAGEBAR_BIN": str(custom),
                   "PATH": "/usr/bin:/bin"}
            subprocess.run([str(SOURCE.parent / "install.sh")], env=env,
                           check=True, capture_output=True, text=True)
            config = home / ".local/share/ai-usagebar/tray/binaries.json"
            self.assertEqual(json.loads(config.read_text())["ai-usagebar"], str(custom))
            launcher = (home / ".local/share/applications/ai-usagebar.desktop").read_text()
            self.assertIn(str(tui), launcher)

    def test_focus_loss_during_open_does_not_close_panel_but_later_loss_does(self):
        window = FakeWindow()
        with patch.object(tray.time, "monotonic", side_effect=[10.0, 10.1, 10.4]), \
             patch.object(tray.GLib, "timeout_add") as schedule:
            tray.UsageDashboard.on_focus_in(window, None, None)
            tray.UsageDashboard.on_focus_out(window, None, None)
            self.assertTrue(window.visible)
            schedule.assert_called_once()
            tray.UsageDashboard.on_focus_out(window, None, None)
            self.assertFalse(window.visible)

    def test_existing_instance_show_command_reaches_server(self):
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "tray.sock")
            server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                server.bind(path)
                server.listen(1)
                app = type("App", (), {"show_window": lambda self: setattr(self, "shown", True)})()
                with patch.object(tray, "SOCKET_PATH", path):
                    self.assertTrue(tray.notify_existing("SHOW"))
                    tray.on_ipc_ready(None, None, server, app)
                self.assertTrue(app.shown)
            finally:
                server.close()

    def test_ipc_activation_uses_x11_server_time(self):
        window = FakeWindow()
        app = type("App", (), {"window": window})()
        with patch.object(tray, "GdkX11") as x11:
            x11.x11_get_server_time.return_value = 12345
            tray.AIUsageTrayApp.show_window(app)
        self.assertEqual(window.presented, [12345])


if __name__ == "__main__":
    unittest.main()
