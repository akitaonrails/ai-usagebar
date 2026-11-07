# Validation

Run checks against the final revision and report PASS, FAIL, NOT RUN or BLOCKED
for each applicable check. A passing test suite does not imply that an installed
desktop app has been updated.

## Shared Rust and frontend contracts

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
make test
cargo machete
make changelog-check
git diff --check
```

Clippy checks Rust types as well as lint rules. `make test` runs Rust tests,
desktop contract suites, Omarchy tests, Linux Mint tests and macOS packaging
tests. Live provider tests remain opt-in because they require real accounts.

When changing the popover sources, also run `npm test` and
`npx tsc --noEmit -p .` from `windows/popover` and inspect the affected styles
in light and dark themes. Native platform behavior needs that platform's host;
browser rendering alone does not verify it.

## Antigravity live-session quota

The regression tests check that a same-account local or verified `agy` reading replaces an
incorrect zero weekly status-line bucket, including reset times and model
pools. Different accounts, Cloud Code readings, missing CLI identity, sign-in
changes during a fetch and stale readings must retain the session's own data.
The CLI's `id_token_email` is hashed for matching; raw email and credentials
are never copied into quota caches or reports.

On macOS, with an enabled Antigravity provider, a usable local server or `agy` and a
status-line session of the same account, exercise the actual report path:

```sh
cargo test --lib report::tests::antigravity_live_account_report_matches_complete_quota --locked -- --ignored --nocapture
```

The test compares the session card's metrics with the complete quota source.
If those prerequisites are unavailable, record this check as NOT RUN or BLOCKED.
After installing a changed app, compare its card with Antigravity's Models &
Quota screen, including remaining percentages, reset times and model pools.
