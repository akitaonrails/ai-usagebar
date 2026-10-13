import http.client
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
from datetime import datetime, timedelta, timezone

import muse_usage as muse


NOW = datetime(2026, 10, 13, 12, tzinfo=timezone.utc)
CANARY = "PrivatePromptAndCredentialCanary"


def record(number, at=NOW, nested=False, **usage):
    event = {"kind": "model_completed", "usage": {
        "input_tokens": 100, "output_tokens": 20, "cached_tokens": 80, **usage}}
    return {"schema_version": 1, "id": f"00000000-0000-0000-0000-{number:012d}",
            "record_type": "event", "payload_type": "agent.run",
            "recorded_at": int(at.timestamp() * 1_000_000),
            "payload": {"event": event} if nested else event,
            "private": CANARY}


class CollectorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def log(self, relative, records, suffix=b""):
        path = self.root / relative / "session.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"".join(json.dumps(item).encode() + b"\n" for item in records) + suffix)
        os.utime(path, (NOW.timestamp(), NOW.timestamp()))
        return path

    def test_counts_completions_and_children_without_double_counting_fork_copies(self):
        first = record(1)
        self.log("2026/09/01/old-active-session", [first, record(2, NOW - timedelta(days=2), nested=True)])
        self.log("2026/10/13/fork", [first])
        self.log("2026/10/13/parent/subagent/child", [record(3, output_tokens=5)])
        result = muse.collect(self.root, NOW)
        self.assertEqual(result["calls"], {"today": 2, "last30": 3})
        self.assertEqual(result["tokens"]["today"], {"input_tokens": 200, "output_tokens": 25, "cached_tokens": 160})
        # Cached tokens are reported separately, never added to input again.
        self.assertNotIn(CANARY, json.dumps(result))

    def test_utc_today_and_rolling_30_days_use_microsecond_record_timestamps(self):
        midnight = NOW.replace(hour=0)
        cutoff = NOW - timedelta(days=30)
        self.log("s", [record(1, midnight), record(2, midnight - timedelta(microseconds=1)),
                       record(3, cutoff), record(4, cutoff - timedelta(seconds=1)),
                       record(5, NOW + timedelta(seconds=1))])
        self.assertEqual(muse.collect(self.root, NOW)["calls"], {"today": 1, "last30": 3})

    def test_corrupt_unfinished_and_invalid_completion_records_are_visible_as_partial(self):
        self.log("s", [record(1), record(2, input_tokens=-1), record(3, cached_tokens=True)],
                 b"not-json\n{\"unfinished\"")
        result = muse.collect(self.root, NOW)
        self.assertEqual(result["calls"]["today"], 1)
        self.assertEqual(result["status"], "Partial: 4 unreadable or unfinished records")

    def test_no_calls_is_distinct_from_missing_logs(self):
        self.assertEqual(muse.collect(self.root, NOW)["calls"]["today"], 0)
        with self.assertRaisesRegex(muse.UsageError, "logs not found"):
            muse.collect(self.root / "absent", NOW)

    def test_unsupported_completion_types_do_not_crash_or_export_the_record(self):
        records = []
        for field, value in (("id", 7), ("schema_version", True), ("recorded_at", "yesterday")):
            item = record(len(records) + 1)
            item[field] = value
            records.append(item)
        item = record(4)
        item["payload"]["usage"] = None
        records.append(item)
        self.log("s", records)
        result = muse.collect(self.root, NOW)
        self.assertEqual(result["calls"]["today"], 0)
        self.assertEqual(result["status"], "Partial: 4 unreadable or unfinished records")
        self.assertNotIn(CANARY, json.dumps(result))

    def test_unrelated_events_and_files_are_not_exported(self):
        self.log("s", [{"record_type": "event", "payload": {"kind": "tool_result", "text": CANARY}},
                       {"record_type": "other", "payload": {"kind": "model_completed"}}])
        (self.root / "auth.json").write_text(CANARY)
        self.assertEqual(muse.collect(self.root, NOW)["status"], "Recorded local completions only")

    def test_line_and_scan_limits_fail_instead_of_returning_zero(self):
        self.log("s", [record(1)])
        for setting in ("MAX_LINE_BYTES", "MAX_SCAN_BYTES"):
            with self.subTest(setting=setting), patch.object(muse, setting, 5):
                with self.assertRaisesRegex(muse.UsageError, "limit"):
                    muse.collect(self.root, NOW)

    def test_discovers_no_symlinked_logs_or_directories(self):
        try:
            path = self.log("real", [record(1)])
            (self.root / "linked-dir").symlink_to(path.parent, target_is_directory=True)
            (self.root / "linked-file").mkdir()
            (self.root / "linked-file/session.jsonl").symlink_to(path)
        except OSError:
            self.skipTest("symlink creation is unavailable on this host")
        self.assertEqual(list(muse.log_files(self.root)), [path])

    @unittest.skipUnless(hasattr(os, "mkfifo"), "named pipes require Unix")
    def test_named_pipe_is_rejected_without_waiting_for_a_writer(self):
        os.mkfifo(self.root / "session.jsonl")
        with self.assertRaisesRegex(muse.UsageError, "not a regular file"):
            muse.collect(self.root, NOW)

    def test_xdg_resolver_uses_only_absolute_override(self):
        with patch.object(muse.Path, "home", return_value=Path("/fixture/home")):
            with patch.dict(os.environ, {"XDG_DATA_HOME": "/fixture/data"}):
                self.assertEqual(muse.default_sessions_path(), Path("/fixture/data/muse/sessions"))
            with patch.dict(os.environ, {"XDG_DATA_HOME": "relative"}):
                self.assertEqual(muse.default_sessions_path(), Path("/fixture/home/.local/share/muse/sessions"))


class ServerTests(unittest.TestCase):
    def test_http_snapshot_missing_logs_errors_and_foreign_hosts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            server = muse.UsageServer(0, root, 60)
            server.snapshot = lambda: muse.collect(root, NOW)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                for path, host, expected in (("/usage", None, 200), ("/other", None, 404),
                                             ("/usage", "untrusted.example", 403)):
                    connection = http.client.HTTPConnection("127.0.0.1", server.server_port, timeout=2)
                    connection.request("GET", path, headers={"Host": host} if host else {})
                    response = connection.getresponse()
                    self.assertEqual(response.status, expected)
                    self.assertEqual(response.getheader("Cache-Control"), "no-store")
                    body = response.read().decode()
                    self.assertNotIn(CANARY, body)
                    self.assertIsInstance(json.loads(body), dict)
                    connection.close()
                server.snapshot = lambda: muse.collect(root / "missing", NOW)
                connection = http.client.HTTPConnection("127.0.0.1", server.server_port, timeout=2)
                connection.request("GET", "/usage")
                response = connection.getresponse()
                self.assertEqual(response.status, 503)
                self.assertNotIn(str(root), response.read().decode())
                connection.close()
            finally:
                server.shutdown()
                server.server_close()
                thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
