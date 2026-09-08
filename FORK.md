# This fork

Public Omarchy-focused fork of
[akitaonrails/ai-usagebar](https://github.com/akitaonrails/ai-usagebar).

- Plugin id: `bradflaugher.ai-usagebar`
- Frontends kept: Omarchy Quattro plugin, `ai-usagebar`, `ai-usagebar-tui`
- Frontends dropped: GNOME, KDE, macOS menu bar, Nix flake, AUR packaging
- Waybar: the CLI can still emit JSON for it; this fork does not document or
  support that path
- Providers: all of them stay in the Rust crate. Enable what you use in
  `~/.config/ai-usagebar/config.toml`.

## Upstream

```bash
git fetch upstream
git merge upstream/main
./scripts/strip-platforms.sh
```

`scripts/strip-platforms.sh` only deletes trees. Manifest id, README, CI, and
Makefile edits will still conflict — keep ours unless the incoming change is a
provider fix you want.

## Install

```bash
omarchy pkg aur add ai-usagebar-bin
omarchy plugin add https://github.com/bradflaugher/ai-usagebar.git --enable
```

The AUR binary is still upstream's. Build from this tree when you change Rust:

```bash
cargo build --release
make install PREFIX=$HOME/.local
```
