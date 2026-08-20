# Encrypted sync — measured calibrations

Numbers the format and the dry-run planner would otherwise have to guess at,
each recorded with the date, the machine, and the exact command that produced
it. Every one is re-derivable: the probes are `#[ignore]`d tests in
`tests/live.rs`, so a plain `cargo test` — including the AUR `check()` on an
installer's machine — runs none of them.

**A stale number here is worse than none.** If a figure looks old, re-run its
command rather than quoting it.

**Nothing below is a promise to a user.** `sync push --dry-run` re-measures at
runtime and reports what it actually found; these are the sanity check on that
machinery, not a substitute for it.

Machine, unless a section says otherwise: Apple M3 Max, macOS 26.5.2,
`aarch64/macos`, release profile.

---

## CAL-4 — the real compressed size of the default bundle

**Measured 2026-08-19.** Probe: `cal4_default_bundle_compressed_size`.

```bash
cargo test --release --test live -- --ignored --nocapture cal4_
# and, to include the opt-in transcripts category without editing config.toml:
AI_USAGEBAR_CAL4_ALL=1 cargo test --release --test live -- --ignored --nocapture cal4_
```

It resolves the real `SyncRoots`, collects through `sync::scope::collect` — the
same collectors the product uses, so this is the actual default bundle and not a
hand-picked sample — reads each file in 256 KiB windows, and runs every window
through `sync::chunk::frame` (zstd level 3, then the power-of-two pad) plus the
40 bytes each seal appends (24-byte nonce, 16-byte Poly1305 tag).

### The default bundle — `config`, `credentials`, `routines`, `chat_index`

| category | files | raw | zstd | stored | raw→stored |
|---|---:|---:|---:|---:|---:|
| `config` | 1 | 183 B | 115 B | 168 B | 1.09x |
| `credentials` | 107 | 23.81 MiB | 8.03 MiB | 10.84 MiB | 2.20x |
| `routines` | 8 | 12.43 KiB | 5.57 KiB | 9.44 KiB | 1.32x |
| `chat_index` | 1533 | 75.60 MiB | 13.47 MiB | 19.22 MiB | 3.93x |
| **TOTAL** | **1649** | **99.42 MiB** | **21.51 MiB** | **30.07 MiB** | **3.31x** |

### With opt-in transcripts as well

| category | files | raw | zstd | stored | raw→stored |
|---|---:|---:|---:|---:|---:|
| `transcripts` | 2077 | 1.99 GiB | 687.16 MiB | 999.80 MiB | 2.04x |
| **TOTAL** | **3726** | **2.09 GiB** | **708.67 MiB** | **1.01 GiB** | **2.08x** |

The transcripts row also reports 2135 files / 1.66 GiB left behind by D3's
bounds. That matches what plan 2-04 measured: the **byte** budget binds before
the 30-day window, and the selection reaches back roughly 21 days rather than 30.
Never render this as "30 days of transcripts" — report what was selected.

### What the two compressed columns mean, and which one to quote

The per-category ratios are genuinely different, so a single blended figure would
mislead:

- `chat_index` is JSON and compresses best, **3.93x** stored.
- `credentials` is SQLite and LevelDB — already-compacted binary — and manages
  only **2.20x**.
- `config` and `routines` are rounding error, and their sub-chunk sizes mean the
  frame's fixed overhead shows up as an unimpressive ratio on a few hundred bytes.

**zstd alone reaches 4.62x on the default bundle** (99.42 MiB → 21.51 MiB), which
is right in the research's assumed 4–5x band. The **stored** figure is 3.31x
(30.07 MiB), because `chunk::frame` rounds each sealed chunk up to a power of two
to keep a ciphertext length from leaking how compressible its plaintext was
(T-02-02). On this bundle that padding plus the per-chunk seal overhead costs
**8.56 MiB — about 40% on top of zstd's output**, and on transcripts it costs
312 MiB, about 45%.

That is not a defect; it is the privacy property being paid for. It *is* a
correction to the estimate: **the research's ~33 MB was arrived at from a ratio
that ignores padding and landed near the right answer by coincidence.** Anything
user-facing must quote the stored column. A `--dry-run` that showed 21.51 MiB
would be wrong by 40% in the user's favour, which is the one direction an
estimate must never be wrong in.

---

## CAL-2 — does a profile's `desktop-state/` churn wholesale?

**Baseline recorded 2026-08-19. Second half deferred — see below.**
Probe: `cal2_desktop_state_chunk_stability`.

### The question, restated after reading the code

The premise this calibration was written from — "Claude Desktop's LevelDB
compaction rewrites the 24 MB profile between app restarts" — does not describe
what the `credentials` category actually carries.

The bundle does not include Claude Desktop's live data directory. It includes
`~/.claude-acc/profiles/<label>/desktop-state/`, which
`claude_desktop::snapshot_profile` writes by staging a fresh copy of the live
`Cookies`, `Cookies-journal`, `Local Storage`, `Session Storage` and `IndexedDB`
into a tempdir and renaming it into place — **only when an account is switched
away from, and only with the app already quit.**

So: quitting and relaunching Claude Desktop does not change one byte of what
gets synced. Measuring across a plain restart would report 0% churn, and would
mean nothing by it. The probe now refuses to record that: an identical tree
prints `INCONCLUSIVE` rather than a reassuring zero.

The question that survives is the one that matters for cost: **when a capture
does rewrite the tree, how much of it is genuinely new bytes?**

### What is already measured, and it is most of the answer

`credentials` does **not** change daily, because captures are not daily. As of
2026-08-19, the four profiles were last captured:

| profile | last capture | days ago |
|---|---|---:|
| `hotmail` | 2026-08-02 | 17 |
| `struct` | 2026-08-04 | 15 |
| `gmail` | 2026-08-07 | 12 |
| `toptal` | 2026-08-17 | 2 |

Four captures in 17 days across four accounts. On the large majority of days the
category contributes **zero** new chunks, whatever its churn rate is on the days
it does move. Its worst case is bounded too: a capture rewrites at most one
profile, so the ceiling is that profile's share — 4.1–8.0 MB raw, under 4 MB
stored at the measured 2.20x — not the full 24 MB.

**Conclusion for the category default: `credentials` stays on.** It cannot
dominate daily sync cost, because on most days it costs nothing, and its
worst day is a few megabytes. Nothing here gates Phase 4's push.

### Deferred verification — the churn rate on a capture day

Still unmeasured: on the day a capture *does* happen, what fraction of the
rewritten tree is new bytes. It only bounds how big that few-megabyte day is, so
it changes no default — but it is the number `sync status` should eventually
quote, and the fallback in the meantime is D-CAL-2's: **report the category's
real churn in `sync status` from index data after a week of use**, rather than
predicting it.

Baselines for all four profiles were recorded on 2026-08-19 at
`~/.cache/ai-usagebar-cal2/<label>.json` (99 files, 179 windows, 24.95 MB total).
To finish the measurement, after Claude Desktop has been used on an account and
then **switched away from** — the switch is the capture — run:

```bash
AI_USAGEBAR_CAL2_PROFILE=~/.claude-acc/profiles/<label>/desktop-state \
AI_USAGEBAR_CAL2_SNAPSHOT=~/.cache/ai-usagebar-cal2/<label>.json \
  cargo test --release --test live -- --ignored --nocapture cal2_
```

It prints total windows, unchanged, changed, bytes changed, and any file that
appeared or disappeared. Nothing needs to be quit or restarted for the probe's
sake; it only ever reads. If the output says `INCONCLUSIVE`, no capture happened
between the two runs and the number is not usable.

---

## Where the other two live

CAL-1 (does a private release asset honour `Range:`) and CAL-3 (Argon2id timing
at the shipped parameters) are Phase 1's, and their probes carry their own
answers and setup in `tests/live.rs`. CAL-3 is measured; CAL-1 was offered again
in Phase 3 and declined, so its 32 MiB fallback still stands unmeasured and is
now only an optimisation question. See `docs/sync-format.md` §7 for both, and
for the open `permissions.admin` question Phase 3 left beside them.
