# Muse Code local usage

The bundled counter reads Muse Code's retained completion logs and feeds the
existing `[[custom]]` provider. It reports input, output, and cached token
counters for today (UTC), today's completed calls, and rolling 30-day counters.
Subagent logs are included and repeated event IDs are counted once.

These figures cover this computer's retained logs across accounts and models.
Deleted logs, other computers, and runs with logging disabled are absent.
The three token counters stay separate because cache accounting depends on the
provider; they cannot reliably determine an invoice or subscription allowance.
There is no invented cost, reset time, or quota percentage.

## Setup

Python 3.9+ is sufficient; there are no packages to install. From this checkout:

```bash
python3 scripts/muse_usage.py --once  # inspect a snapshot without a server
python3 scripts/muse_usage.py         # keep running while ai-usagebar is open
```

The default log directory is
`${XDG_DATA_HOME:-~/.local/share}/muse/sessions`, following Muse's Unix layout
on Linux/macOS. A relative `XDG_DATA_HOME` is ignored. For another location,
including a mounted Windows session directory, pass it explicitly:

```bash
python3 scripts/muse_usage.py --sessions-path /path/to/muse/sessions
```

Uncomment the complete **Muse Code local token usage** recipe in
[config.example.toml](../config.example.toml) and append it to your own
`config.toml`. The server binds only to `127.0.0.1:41837`; `--port` changes it
and the recipe's URL must match. `api_key = "local"` is a placeholder required
by the existing custom-provider configuration, not a Muse credential.

Open the TUI or read the generic report:

```bash
ai-usagebar-tui
ai-usagebar usage --json
```

The report entry is `custom:muse`. Frontends consuming this report can render
its text rows. The single-vendor `--vendor` widget selector accepts built-in
vendors only; `--vendor muse` is not a selector for this custom provider.

Snapshots refresh on demand at most once per minute (change
`--refresh-seconds` for the counter and `cache_ttl_secs` in the recipe).
Stopping the counter makes the custom provider fall back to its normal cache;
ai-usagebar marks that data stale. No service is installed automatically.

## Data and failure handling

Only `session.jsonl` files are read, with no discovered symlinks followed. The
counter accepts both published completion-envelope layouts and deduplicates
by event ID. It reads completion times from `recorded_at` in Unix microseconds,
so a session started yesterday can still contribute calls to today.
Unmodified logs older than 30 days are skipped.

No raw events, prompts, credential files, or account identity leave the reader,
and nothing is written to the session directory. The HTTP response contains
only aggregate counters, a timestamp, a source label, and scan status. The
server rejects foreign Host headers and supplies no browser CORS permission.
Other local processes can read these aggregate counters.

Corrupt, unsupported, or unfinished records are omitted and identified by the
**Log status** row as a partial result. A later refresh picks up a completed
trailing record. A missing/unreadable directory, failed file read, or scan limit
returns HTTP 503 instead of a successful zero. Limits are 1 MiB per line,
256 MiB read per scan, 10,000 log files, and 100,000 directory entries. An
explicit `--sessions-path` can select a smaller tree if necessary.

## Subscription quota research

Muse 1.4.0 added the official MSP `usage/read` and `usage/changed` methods,
which carry last-observed session and weekly subscription percentages.
The tested Muse 1.4.7 fresh hosts returned no observation, including after
`model/list` and repeated reads without any prompt. A reliable retrieval path
for a standalone quota provider still needs real subscription-account
validation. [Research and effort estimate](muse-code-research.md) records the
exact protocol, pinned first-party schemas, and read-only probe results.

The log parser follows Meta's
[completion fixture](https://github.com/meta-models/meta-model-cookbook/blob/0a99b987858ec4e49c04fc3f114bf33851e81deb/04_muse_code/02_deterministic_replay/golden-traces/checkout_probe.json)
and [audit-log recipe](https://github.com/meta-models/meta-model-cookbook/blob/0a99b987858ec4e49c04fc3f114bf33851e81deb/04_muse_code/01_append_only_audit_log/README.md).
