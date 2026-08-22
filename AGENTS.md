# ai-usagebar Agent Guide

## Architecture

- Rust 2024 crate; the declared minimum Rust version is 1.88. `src/lib.rs` is the shared core, while `src/bin/ai-usagebar.rs` and `src/bin/ai-usagebar-tui.rs` are thin entrypoints.
- Provider-specific wire types, fetching, and rendering live in `src/<vendor>/`; shared usage types are in `src/usage.rs`. Keep provider behavior in Rust, not in desktop frontends.
- `src/widget/` owns the Waybar widget, `src/tui/` owns the terminal user interface (TUI), and `src/report.rs` owns the `ai-usagebar usage --json` contract consumed by all desktop frontends.
- `omarchy/`, `gnome-extension/`, `kde-plasmoid/`, and `macos/` are separate presentation adapters. The Omarchy plugin runs only fixed binary commands; it must not fetch providers, manage credentials, or duplicate Rust configuration logic.

## Required Contracts

- The widget binary must always emit valid fallback warning output and exit `0`; Waybar hides modules that exit nonzero. `ai-usagebar usage` is the administrative report and intentionally returns meaningful failures.
- Keep caches atomic and lock-protected. Named Anthropic and OpenRouter accounts require separate cache directories so one account cannot serve another account's data.
- Treat `usage --json` as an additive public interface. Preserve ordered `sections`, real non-percentage rows, `severity`, and absolute `reset_at` data; build report metrics through `SectionBuilder::push_metric` rather than recreating vendor ordering in `report.rs` or a frontend.
- Use `VendorId::display_name()` as the canonical label source. Frontends may add local context but must not maintain a full provider-name table.
- KDE (K Desktop Environment) Plasma selects report entries client-side from one `usage --json` response. Never add `--vendor` to its fetch command: it would collide with Waybar's shared active-vendor state. Its JavaScript runs under the Qt Modeling Language (QML) V4 engine, so avoid optional catch bindings (`catch {}`) and Unicode property escapes (`\p{...}`).

## Verification

- Baseline local suite: `make test` runs Rust tests plus GNOME, KDE Plasma, and Omarchy model tests.
- Continuous integration parity: `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --all-targets --locked`, `make desktop-test`, and `cargo machete`.
- Focused Rust example: `cargo test --test anthropic_e2e full_response_renders_expected_waybar_json`.
- Run the affected adapter test directly: `node omarchy/model.test.mjs`, `node gnome-extension/marker-logic.test.mjs`, or `node kde-plasmoid/plasmoid-logic.test.mjs`.
- For KDE Plasma QML changes, run `make qml-lint` and `make qml-test` when Qt 6 tools are installed. `make mjs-probe` additionally requires `plasma-sdk` and a running Plasma session.
- For macOS menu-bar changes, run `./macos/run-tests.sh` on macOS.
- Live vendor checks are intentionally ignored and need shell credentials: `cargo test --test live <provider> -- --ignored --nocapture`. Do not run or alter them to depend on real credentials during ordinary tests.

## Test And Secret Discipline

- Tests must be hermetic: never read or write real home, XDG (X Desktop Group) cache, config, credential, transcript, or Omarchy paths. Inject temporary paths and dependencies, such as `Cache::at`, `*_at`, and `read_from`; keep live tests behind `#[ignore]`.
- Never commit or print credentials. Do not dump user `config.toml`, OAuth files, or environment variables; use temporary fixtures and redact diagnostic output.

## Releases

- Read `CLAUDE.md` before release or packaging work. Its checklist is authoritative, including exact Arch User Repository (AUR) `.SRCINFO` regeneration commands.
- Release validation requires the annotated `vX.Y.Z` tag on `main`, `Cargo.toml`, root `manifest.json`, both PKGBUILDs, both `.SRCINFO` files, and `CHANGELOG.md` to agree.
- A change under `kde-plasmoid/` also requires bumping its independent `KPlugin.Version` in `kde-plasmoid/package/metadata.json`.
- Never force-move or replace a published release tag; cut a new patch release instead.
