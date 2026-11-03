"""Hermetic contracts for COSMIC integration; no GUI, credentials or network."""
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from tray_model import select_tray_backend

ROOT = Path(__file__).resolve().parent.parent
loader = importlib.machinery.SourceFileLoader('cosmic_launcher', str(ROOT / 'pop-os/ai-usagebar-cosmic-tray'))
spec = importlib.util.spec_from_loader(loader.name, loader)
launcher = importlib.util.module_from_spec(spec)
loader.exec_module(launcher)


class CosmicContractTest(unittest.TestCase):
    def test_cosmic_uses_appindicator_under_wayland_and_x11(self):
        for desktop in ('COSMIC', 'cosmic', 'pop:COSMIC'):
            for session in ('wayland', 'x11', ''):
                self.assertEqual(select_tray_backend(desktop, session, True, True), 'appindicator')

    def test_cosmic_does_not_claim_an_invisible_x11_icon_without_indicator(self):
        self.assertIsNone(select_tray_backend('COSMIC', 'x11', True, False))

    def test_mint_and_other_desktops_keep_their_backends(self):
        self.assertEqual(select_tray_backend('X-Cinnamon:Cinnamon', 'x11', True, True), 'xapp')
        self.assertEqual(select_tray_backend('X-Cinnamon', 'x11', True, True), 'xapp')
        self.assertEqual(select_tray_backend('XFCE', 'x11', False, True), 'gtk')
        self.assertEqual(select_tray_backend('GNOME', 'wayland', False, True), 'appindicator')
        self.assertIsNone(select_tray_backend('GNOME', 'wayland', False, False))

    def test_launcher_uses_fixed_interpreter_and_preserves_args_without_shell(self):
        calls = []
        env = {'XDG_CURRENT_DESKTOP': 'GNOME', 'SOME_OPTION': 'kept'}
        launcher.launch(env, '/tmp/home with spaces', ['--window'], lambda *args: calls.append(args))
        executable, argv, child_env = calls[0]
        self.assertEqual(executable, '/usr/bin/python3')
        self.assertEqual(argv, ['/usr/bin/python3', '/tmp/home with spaces/.local/bin/ai-usagebar-tray', '--window'])
        self.assertEqual(child_env['XDG_CURRENT_DESKTOP'], 'COSMIC')
        self.assertTrue(child_env['AI_USAGEBAR_TRAY_ICON'].endswith('ai-usagebar-symbolic.svg'))
        self.assertEqual(env['XDG_CURRENT_DESKTOP'], 'GNOME')

    def test_installer_reuses_dashboard_and_sets_cosmic_launchers(self):
        with tempfile.TemporaryDirectory(prefix='cosmic install ') as directory:
            home = Path(directory)
            tools = home / 'test-tools'; tools.mkdir()
            python = tools / 'python3'
            # Only GI preflight is stubbed. Rendering/install logic uses real Python.
            python.write_text('#!/bin/sh\nif [ "$1" = "-c" ]; then exit 0; fi\nexec ' + sys.executable + ' "$@"\n')
            python.chmod(0o755)
            cli = home / 'custom binaries/ai-usagebar'
            tui = home / 'custom binaries/ai-usagebar-tui'
            for binary in (cli, tui):
                binary.parent.mkdir(parents=True, exist_ok=True)
                binary.write_text('#!/bin/sh\nexit 0\n'); binary.chmod(0o755)
            env = {'HOME': str(home), 'PATH': str(tools) + ':/usr/bin:/bin',
                   'AI_USAGEBAR_BIN': str(cli), 'AI_USAGEBAR_TUI_BIN': str(tui)}
            subprocess.run(['bash', str(ROOT / 'pop-os/install.sh')], env=env, check=True, capture_output=True)
            self.assertEqual((home / '.local/bin/ai-usagebar-tray').read_bytes(), (ROOT / 'linux-mint/ai-usagebar-tray').read_bytes())
            for file in ('.local/share/applications/ai-usagebar.desktop', '.config/autostart/ai-usagebar-tray.desktop'):
                content = (home / file).read_text()
                self.assertIn('ai-usagebar-cosmic-tray', content)
                self.assertNotIn('@TRAY_EXEC@', content)
            config = json.loads((home / '.local/share/ai-usagebar/tray/binaries.json').read_text())
            self.assertEqual(config['ai-usagebar'], str(cli))
            self.assertTrue((home / '.local/bin/ai-usagebar-cosmic-tray').stat().st_mode & 0o111)
            self.assertIn('#ffffff', (home / '.local/share/ai-usagebar/tray/ai-usagebar-symbolic.svg').read_text())

    def test_missing_ayatana_fails_before_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory); tools = home / 'tools'; tools.mkdir()
            python = tools / 'python3'
            python.write_text('#!/bin/sh\ncase "$2" in *Ayatana*) exit 1;; *) exit 0;; esac\n')
            python.chmod(0o755)
            result = subprocess.run(['bash', str(ROOT / 'pop-os/install.sh')],
                                    env={'HOME': directory, 'PATH': str(tools) + ':/usr/bin:/bin'},
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('gir1.2-ayatanaappindicator3-0.1', result.stderr)
            self.assertFalse((home / '.local').exists())


if __name__ == '__main__':
    unittest.main()
