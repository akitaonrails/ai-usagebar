"""Exercise the documented WSL snippets without a login, WSL, or a server."""

import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import textwrap
import unittest


def snippet(language):
    readme = Path(__file__).resolve().parents[1] / "windows" / "README.md"
    recipe = readme.read_text(encoding="utf-8").split(
        "## Accounts signed in inside WSL\n", 1
    )[1]
    code = recipe.split(f"```{language}\n", 1)[1].split("```", 1)[0]
    return textwrap.dedent(code)


def metric(percent, window):
    return {
        "label": "Same pool",
        "window_secs": window,
        "percent": percent,
        "reset_at": "2026-11-27T00:00:00Z",
        "detail": "Do not publish my extra fields",
    }


def entry(entry_id, *metrics):
    return {"id": entry_id, "plan": "Plus", "stale": False, "metrics": metrics}


class WslRecipeTest(unittest.TestCase):
    def test_filter_selects_entries_and_keeps_equal_labels_in_distinct_windows(self):
        work = entry("openai@work", metric(42, 18000), metric(71, 604800))
        document = {"entries": [entry("openai@personal"), work]}
        result = subprocess.run(
            [sys.executable, "-I", "-c", snippet("python"), "openai@work"],
            input=json.dumps(document),
            capture_output=True,
            text=True,
            check=True,
        )
        entries = json.loads(result.stdout)["entries"]
        self.assertEqual(list(entries), ["openai@work"])
        windows = entries["openai@work"]["metrics"]["ungrouped"]
        self.assertEqual(windows["18000"]["Same pool"], {
            "percent": 42, "reset_at": "2026-11-27T00:00:00Z"
        })
        self.assertEqual(windows["604800"]["Same pool"]["percent"], 71)

    @unittest.skipUnless(sys.platform.startswith("linux"), "publisher targets WSL Linux")
    def test_publisher_keeps_private_snapshot_when_report_or_filter_fails(self):
        work = entry("openai@work", metric(42, 18000))
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            (home / "aiub-wsl-rekey.py").write_text(snippet("python"), encoding="utf-8")
            cli = home / "ai-usagebar"
            cli.write_text(
                '#!/bin/sh\nprintf "%s" "$REPORT"\nexit "$REPORT_STATUS"\n',
                encoding="utf-8",
            )
            cli.chmod(0o700)

            def publish(entries, status="0"):
                return subprocess.run(
                    ["bash", "-c", snippet("bash")],
                    cwd=home,
                    env={
                        "HOME": directory,
                        "PATH": f"{directory}:{os.defpath}",
                        "REPORT": json.dumps({"entries": entries}),
                        "REPORT_STATUS": status,
                    },
                    capture_output=True,
                    text=True,
                )

            first = publish([work])
            self.assertEqual(first.returncode, 0, first.stderr)
            root = home / ".local" / "state" / "aiub-wsl"
            public = root / "public"
            snapshot = public / "usage.json"
            original = snapshot.read_bytes()
            self.assertEqual(stat.S_IMODE(root.stat().st_mode), 0o700)
            self.assertEqual(stat.S_IMODE(public.stat().st_mode), 0o700)
            self.assertEqual(stat.S_IMODE(snapshot.stat().st_mode), 0o600)
            self.assertNotEqual(publish([work], status="1").returncode, 0)
            self.assertNotEqual(publish([work, work]).returncode, 0)
            ambiguous = entry("openai@work", metric(42, 18000), metric(71, 18000))
            self.assertNotEqual(publish([ambiguous]).returncode, 0)
            self.assertEqual(snapshot.read_bytes(), original)
            self.assertEqual(list(root.iterdir()), [public])


if __name__ == "__main__":
    unittest.main()
