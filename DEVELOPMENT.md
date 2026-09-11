# Development guide

Build, test, and run `ai-usagebar` from a checkout. For the pre-PR gate and
provider-addition rules see [CONTRIBUTING.md](CONTRIBUTING.md). For release
invariants see [CLAUDE.md](CLAUDE.md).

- [Prerequisites](#prerequisites)
- [Build](#build)
- [Run locally](#run-locally)
- [Tests](#tests)
- [Lint and format](#lint-and-format)
- [Configuration while developing](#configuration-while-developing)
- [Ollama Cloud](#ollama-cloud)
- [Windows](#windows)
- [Layout](#layout)
- [See also](#see-also)

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Rust / Cargo | **1.88+** (MSRV) | [rustup](https://rustup.rs/) |
| Node.js | 18+ | GNOME / KDE / Omarchy / Windows popover contract tests |
| Git | any | |

On Windows you also need the **MSVC Build Tools** (linker) and **NASM**
(`ring` uses it). See [docs/windows-build.md](docs/windows-build.md).

Optional:

- `cargo-machete` — unused-dependency check (`cargo install cargo-machete`)
- Live API keys in the environment for `make smoke`

## Build

```bash
git clone https://github.com/akitaonrails/ai-usagebar.git
cd ai-usagebar

cargo build --release
```

Binaries land at:

| Binary | Path |
|---|---|
| Widget / CLI | `target/release/ai-usagebar` |
| TUI | `target/release/ai-usagebar-tui` |
| Windows tray | `target/release/ai-usagebar-tray` (Windows only) |

Debug builds (`cargo build`) are fine for iterating; the first release build
is slow because of `reqwest` / `ring`.

Install to `$PREFIX` (default `/usr/local`):

```bash
make install                 # /usr/local/bin + share
make install PREFIX=$HOME/.local
```

KDE plasmoid install is separate (`make install install-plasmoid`) and is
documented in [kde-plasmoid/README.md](kde-plasmoid/README.md).

## Run locally

From the checkout, without installing:

```bash
./target/release/ai-usagebar --json
./target/release/ai-usagebar --vendor openrouter --format '{or_balance}'
./target/release/ai-usagebar --watch 5          # refresh every 5s while iterating on --format
./target/release/ai-usagebar-tui
```

`--config PATH` points both binaries at an alternate TOML. The file **must
already exist**; loads and the Settings overlay then read and write that path
for the whole process, so a scratch config never touches the real one:

```bash
./target/release/ai-usagebar --config ./config.test.toml --vendor kimi --watch 5
./target/release/ai-usagebar-tui --config ./config.test.toml
```

Default config locations:

| OS | Path |
|---|---|
| Linux | `~/.config/ai-usagebar/config.toml` |
| macOS | `~/Library/Application Support/ai-usagebar/config.toml` (legacy `~/.config/…` still wins if present) |
| Windows | `%APPDATA%\ai-usagebar\config.toml` |

## Tests

The gate contributors run is the same one CI runs:

```bash
make test                                          # cargo test + desktop JS suites
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
cargo machete                                      # no unused dependencies
```

`make test` rather than `cargo test` alone: the GNOME, KDE, Omarchy, and
Windows-popover frontends have Node contract tests, and a report-shape change
can break them without touching Rust.

| Target | What it runs |
|---|---|
| `make test` | `cargo test` + `desktop-test` + `plugin-test` |
| `make desktop-test` | GNOME, KDE, Windows popover `.test.mjs` |
| `make plugin-test` | Omarchy `model.test.mjs` |
| `make smoke` | `cargo test --test live -- --ignored --nocapture` |
| `make clippy` | `cargo clippy --all-targets -- -D warnings` |
| `make fmt` | `cargo fmt` |

Live smoke tests need credentials in the environment (`OLLAMA_API_KEY`,
`ZAI_API_KEY`, …). Tests marked `#[ignore]` never run in `make test`. A
`#[test]` must not read a real `$HOME` / `$XDG` path — the AUR package runs
`cargo test` during install.

```bash
# One live vendor
OLLAMA_API_KEY=… cargo test --test live ollama_live -- --ignored --nocapture
```

## Lint and format

```bash
cargo fmt --all
cargo clippy --all-targets --locked -- -D warnings
```

CI denies warnings (`-D warnings`) and checks formatting. rustfmt is the
style; do not hand-format around it.

## Configuration while developing

Copy [config.example.toml](config.example.toml) and enable only the vendors
you have credentials for. Opt-in vendors (DeepSeek, Ollama Cloud, Kimi, …)
default to `enabled = false` and never fetch until flipped on.

```toml
[ollama]
enabled = true
api_key_env = "OLLAMA_API_KEY"
plan = "pro"
```

Putting an inline `api_key` in any section requires `chmod 600` on the file.
Environment variables are the safer default.

See [docs/configuration.md](docs/configuration.md) for the full reference.

## Ollama Cloud

Native provider — **not** a `[[custom]]` table. Bearer token from
https://ollama.com/settings/keys against `GET https://ollama.com/api/usage`.

Copy the `[ollama]` section from [config.example.toml](config.example.toml)
into your config and set `enabled = true`:

```toml
[ollama]
enabled = true
api_key_env = "OLLAMA_API_KEY"
plan = "pro"
```

```bash
export OLLAMA_API_KEY="…"          # minted at ollama.com/settings/keys
./target/release/ai-usagebar --vendor ollama
./target/release/ai-usagebar-tui
```

The local daemon at `127.0.0.1:11434` has **no** quota route. The Ed25519
CLI key in `~/.ollama/id_ed25519` is a registry credential and is refused by
`/api/usage` with 401.

Full walkthrough: [docs/ollama-setup.md](docs/ollama-setup.md).

## Windows

MSVC + NASM are required to compile. After `cargo build --release`:

```powershell
.\target\release\ai-usagebar.exe --json
.\target\release\ai-usagebar-tui.exe
```

There is no `make` on a stock PowerShell. Run the cargo commands directly.
Details, PATH, and tray install: [docs/windows-build.md](docs/windows-build.md).

## Layout

| Path | What |
|---|---|
| `src/` | Library + vendor modules (`src/ollama/`, `src/zai/`, …) |
| `src/bin/` | `ai-usagebar`, `ai-usagebar-tui`, `ai-usagebar-tray` |
| `src/tui/` | TUI app, panels, Settings overlay |
| `src/widget/` | Waybar / CLI renderer |
| `tests/` | Integration, live smoke, fixtures |
| `gnome-extension/`, `kde-plasmoid/`, `omarchy/`, `macos/`, `windows/` | Native frontends |
| `docs/` | Configuration, placeholders, vendor endpoints |
| `.github/workflows/` | `ci.yml` (fmt, clippy, tests, MSRV 1.88, Nix) and `release.yml` |

Adding a vendor is an exhaustive-match exercise: `VendorId`, `VendorSnapshot`,
`VendorId::all()`, config section, catalog, detect, widget CLI, TUI fetch,
Settings `KEY_VENDORS` (if API-key), placeholders, changelog. Missing
`VendorId::all()` is how a provider shows up in Settings and nowhere else.

## See also

- [CONTRIBUTING.md](CONTRIBUTING.md) — pre-PR gate and how to add a provider
- [CLAUDE.md](CLAUDE.md) — release checklist and invariants
- [docs/configuration.md](docs/configuration.md) — config reference
- [docs/vendor-endpoints.md](docs/vendor-endpoints.md) — endpoint matrix
- [docs/windows-build.md](docs/windows-build.md) — Windows toolchain
- [docs/ollama-setup.md](docs/ollama-setup.md) — Ollama Cloud
