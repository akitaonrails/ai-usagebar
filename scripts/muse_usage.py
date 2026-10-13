#!/usr/bin/env python3
"""Read Muse Code's local completion counters for an ai-usagebar custom provider.

No credentials, prompts, model calls, prices, or subscription quotas are exported.
See docs/muse-code.md for setup and the first-party log schema references.
"""

import argparse
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import stat
import sys
import time
import uuid

MAX_LINE_BYTES = 1024 * 1024
MAX_SCAN_BYTES = 256 * 1024 * 1024
MAX_FILES = 10_000
MAX_ENTRIES = 100_000


class UsageError(Exception):
    """An incomplete scan must not appear as a successful zero counter."""


def default_sessions_path():
    xdg = os.environ.get("XDG_DATA_HOME", "")
    base = Path(xdg) if xdg and Path(xdg).is_absolute() else Path.home() / ".local/share"
    return base / "muse/sessions"


def completion(record):
    """Return only identity, arrival time, and token counters from an envelope."""
    if not isinstance(record, dict) or record.get("record_type") != "event":
        return None
    payload = record.get("payload")
    if not isinstance(payload, dict):
        return None
    event = payload.get("event", payload)
    if not isinstance(event, dict) or event.get("kind") != "model_completed":
        return None
    try:
        if type(record.get("schema_version")) is not int or record["schema_version"] != 1:
            raise ValueError
        if not isinstance(record["id"], str):
            raise ValueError
        event_id = str(uuid.UUID(record["id"]))
        stamp = record["recorded_at"]
        if type(stamp) is not int or stamp <= 0:
            raise ValueError
        at = datetime(1970, 1, 1, tzinfo=timezone.utc) + timedelta(microseconds=stamp)
        usage = event["usage"]
        counters = tuple(usage[key] for key in ("input_tokens", "output_tokens", "cached_tokens"))
        if any(type(value) is not int or value < 0 for value in counters):
            raise ValueError
        return event_id, at, counters
    except (KeyError, TypeError, ValueError, OverflowError, OSError):
        raise UsageError("Muse completion has an unsupported log schema") from None


def log_files(root):
    """Include child logs and old sessions still running; skip discovered links."""
    entries = files = 0

    def on_error(_error):
        raise UsageError("Muse session directory could not be read") from None

    for directory, dirs, names in os.walk(root, followlinks=False, onerror=on_error):
        entries += len(dirs) + len(names)
        if entries > MAX_ENTRIES:
            raise UsageError("Muse log directory exceeded the entry limit")
        dirs[:] = [name for name in dirs if not (Path(directory) / name).is_symlink()]
        if "session.jsonl" in names:
            path = Path(directory) / "session.jsonl"
            if path.is_symlink():
                continue
            files += 1
            if files > MAX_FILES:
                raise UsageError("Muse log scan exceeded the file limit")
            yield path


def collect(root, now):
    if not root.is_dir():
        raise UsageError("Muse session logs not found; use --sessions-path or run Muse with logging enabled")
    cutoff = now - timedelta(days=30)
    today = now.replace(hour=0, minute=0, second=0, microsecond=0)
    totals = {key: dict(calls=0, input_tokens=0, output_tokens=0, cached_tokens=0)
              for key in ("today", "last30")}
    seen = set()
    scanned = skipped = 0
    for path in log_files(root):
        try:
            # O_NOFOLLOW closes the final-component symlink race on Unix.
            flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
            fd = os.open(path, flags)
            with os.fdopen(fd, "rb") as stream:
                info = os.fstat(stream.fileno())
                if not stat.S_ISREG(info.st_mode):
                    raise UsageError("Muse session log is not a regular file")
                if info.st_mtime < cutoff.timestamp():
                    continue
                while True:
                    line = stream.readline(MAX_LINE_BYTES + 1)
                    if not line:
                        break
                    scanned += len(line)
                    if scanned > MAX_SCAN_BYTES or len(line) > MAX_LINE_BYTES:
                        raise UsageError("Muse log scan exceeded the byte or line limit")
                    if not line.endswith(b"\n"):
                        # An active writer may not have finished its last record.
                        skipped += 1
                        break
                    try:
                        item = completion(json.loads(line))
                    except (ValueError, UnicodeDecodeError, RecursionError, UsageError):
                        skipped += 1
                        continue
                    if item is None:
                        continue
                    event_id, at, counters = item
                    if not cutoff <= at <= now or event_id in seen:
                        continue
                    seen.add(event_id)
                    for key, start in (("today", today), ("last30", cutoff)):
                        if at >= start:
                            totals[key]["calls"] += 1
                            for field, value in zip(("input_tokens", "output_tokens", "cached_tokens"), counters):
                                totals[key][field] += value
        except OSError:
            raise UsageError("Muse session log could not be read") from None
    return {
        "source": "Muse Code local logs",
        "as_of": now.isoformat(),
        "status": (f"Partial: {skipped} unreadable or unfinished records"
                   if skipped else "Recorded local completions only"),
        "tokens": {key: {field: count for field, count in value.items() if field != "calls"}
                   for key, value in totals.items()},
        "calls": {key: value["calls"] for key, value in totals.items()},
    }


class UsageServer(HTTPServer):
    def __init__(self, port, root, refresh_seconds):
        # Deliberately no public-bind override: the data belongs to this machine.
        super().__init__(("127.0.0.1", port), UsageHandler)
        self.root = root
        self.refresh_seconds = refresh_seconds
        self.cached = None
        self.cached_at = 0.0

    def snapshot(self):
        if self.cached is None or time.monotonic() - self.cached_at >= self.refresh_seconds:
            result = collect(self.root, datetime.now(timezone.utc))
            self.cached = result
            self.cached_at = time.monotonic()
        return self.cached


class UsageHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        # Reject foreign Host headers so browser DNS rebinding cannot read logs.
        if self.headers.get("Host") != f"127.0.0.1:{self.server.server_port}":
            self.reply(403, {"error": "Loopback Host required"})
        elif self.path != "/usage":
            self.reply(404, {"error": "Use /usage"})
        else:
            try:
                self.reply(200, self.server.snapshot())
            except UsageError as error:
                self.reply(503, {"error": str(error)})

    def reply(self, status, payload):
        body = json.dumps(payload, allow_nan=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *_args):
        # No request URLs or local paths in HTTP logs.
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sessions-path", type=Path, help="Muse sessions root (default: XDG data directory/muse/sessions)")
    parser.add_argument("--port", type=int, default=41837)
    parser.add_argument("--refresh-seconds", type=int, default=60)
    parser.add_argument("--once", action="store_true", help="print a snapshot and exit without starting a server")
    args = parser.parse_args()
    if not 1 <= args.port <= 65535 or not 1 <= args.refresh_seconds <= 3600:
        parser.error("port must be 1..65535 and refresh-seconds must be 1..3600")
    root = (args.sessions_path or default_sessions_path()).expanduser()
    try:
        if args.once:
            print(json.dumps(collect(root, datetime.now(timezone.utc))))
        else:
            with UsageServer(args.port, root, args.refresh_seconds) as server:
                print(f"Muse local usage: http://127.0.0.1:{args.port}/usage", file=sys.stderr)
                server.serve_forever()
    except UsageError as error:
        print(str(error), file=sys.stderr)
        return 1
    except OSError:
        print("Muse usage server could not start; check the port and log access", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
