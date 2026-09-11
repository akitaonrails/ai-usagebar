# Building and running on Windows

Native Windows build of the widget, TUI, and system-tray binaries.

- [Prerequisites](#prerequisites)
- [Build](#build)
- [Run](#run)
- [Configuration](#configuration)
- [Install](#install)
- [Troubleshooting](#troubleshooting)

The cross-platform workflow (tests, clippy, layout) lives in
[DEVELOPMENT.md](../DEVELOPMENT.md). This page is the Windows-specific
toolchain and PATH.

## Prerequisites

| Tool | Why | Install |
|---|---|---|
| **Rust 1.88+** | MSRV | [rustup.rs](https://rustup.rs/) (`winget install Rustlang.Rustup`) |
| **MSVC Build Tools 2022** | linker (`link.exe`) | `winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"` |
| **NASM** | `ring` compiles assembly | `winget install NASM.NASM` |
| **Git** | clone | optional |

After installing, **open a new terminal** so `cargo` and `nasm` are on `PATH`.
NASM often lands in `%LOCALAPPDATA%\bin\NASM`; rustup in `%USERPROFILE%\.cargo\bin`.

```powershell
rustc --version    # 1.88 or newer
where.exe nasm
where.exe link
```

## Build

```powershell
cd path\to\ai-usagebar
cargo build --release
```

| Binary | Path |
|---|---|
| Widget / CLI | `target\release\ai-usagebar.exe` |
| TUI | `target\release\ai-usagebar-tui.exe` |
| Tray | `target\release\ai-usagebar-tray.exe` |

First release build is several minutes; rebuilds are seconds.

There is no `make` on a stock PowerShell. Equivalent cargo commands:

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Node contract tests (optional, needs Node 18+):

```powershell
node gnome-extension\marker-logic.test.mjs
node kde-plasmoid\plasmoid-logic.test.mjs
node windows\popover\popover.test.mjs
node omarchy\model.test.mjs
```

## Run

```powershell
.\target\release\ai-usagebar.exe --json
.\target\release\ai-usagebar.exe --vendor ollama --watch 5
.\target\release\ai-usagebar-tui.exe
```

`--config` points both binaries at an alternate TOML. The file **must already
exist**; Settings then reads and writes that path for the whole process:

```powershell
.\target\release\ai-usagebar-tui.exe
```

TUI keys: `Tab` / `h` `l` cycle tabs, `r` refresh, `s` Settings, `q` quit.

## Configuration

Default file: `%APPDATA%\ai-usagebar\config.toml`.

```powershell
$dir = "$env:APPDATA\ai-usagebar"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item config.example.toml "$dir\config.toml"
notepad "$dir\config.toml"
```

API keys belong in the environment, not in the file:

```powershell
# session only
$env:OLLAMA_API_KEY = "…"

# persist for this user
[System.Environment]::SetEnvironmentVariable(
    "OLLAMA_API_KEY",
    "…",
    [System.EnvironmentVariableTarget]::User
)
```

Restart the terminal after a permanent set.

Ollama Cloud walkthrough: [ollama-setup.md](./ollama-setup.md).

## Install

Copy the binaries somewhere on `PATH`, for example:

```powershell
$bin = "$env:LOCALAPPDATA\ai-usagebar\bin"
New-Item -ItemType Directory -Force -Path $bin | Out-Null
Copy-Item target\release\ai-usagebar.exe, target\release\ai-usagebar-tui.exe $bin
# add $bin to the user PATH, or invoke with the full path
```

The tray app is documented in [windows/README.md](../windows/README.md).

## Troubleshooting

**`cargo` / `nasm` / `link` not found** — new terminal after winget; confirm
`%USERPROFILE%\.cargo\bin` and `%LOCALAPPDATA%\bin\NASM` are on `PATH`.

**`ring` fails to compile** — NASM missing. `winget install NASM.NASM`.

**TUI shows a vendor in Settings but not as a tab** — that vendor is opt-in.
Set `[vendor] enabled = true` in the TOML you passed with `--config`, or pick
it as primary in Settings and Save (which writes `enabled = true`).

**No usage data** — `echo $env:OLLAMA_API_KEY` (or the vendor's env). Empty
means the process cannot authenticate.

**JSON parse / schema errors against a live endpoint** — dump the body and
compare with `docs/vendor-endpoints.md`:

```powershell
Invoke-RestMethod `
  -Uri "https://ollama.com/api/usage" `
  -Headers @{ Authorization = "Bearer $env:OLLAMA_API_KEY" }
```
