# OpenRouter account guide

ai-usagebar can report several OpenRouter keys without running separate config
or cache roots. The existing `[openrouter]` key remains the default account.

## Add named accounts

Add one entry per extra key to `~/.config/ai-usagebar/config.toml`:

```toml
[openrouter]
enabled = true
api_key_env = "OPENROUTER_API_KEY"
# api_key = "sk-or-v1-default"

[[openrouter.accounts]]
label = "work"
api_key_env = "OPENROUTER_WORK_API_KEY"

[[openrouter.accounts]]
label = "personal"
api_key = "sk-or-v1-personal"
```

An account can use `api_key_env`, an inline `api_key`, or both. The environment
variable wins when both are set. If you store any key inline, ai-usagebar
tightens the config file to mode `0600` on Unix.

Labels cannot be empty, contain path separators, drive prefixes, or control
characters, or use a reserved cache filename. Duplicate labels are rejected.

## What one entry separates

Each account entry is one OpenRouter login's key — one entry per workspace is
the pattern for working across several workspaces. Be aware of what it does
*not* separate: keys created inside the same OpenRouter workspace share that
workspace's billing account, so two entries holding two keys from one
workspace both report that workspace's credits and spend. The per-entry split
is per login session (workspace), not per key within a single bill.

## Select an account

Named accounts appear automatically as separate TUI tabs and `usage` report
entries. Select one directly in the widget:

```bash
ai-usagebar --vendor openrouter --account work
```

The default account keeps the original
`~/.cache/ai-usagebar/openrouter/` cache. Named accounts use
`~/.cache/ai-usagebar/openrouter/<label>/`.

To omit the unnamed account from aggregate views when all keys are named:

```toml
[openrouter]
show_default_account = false
```

This setting does not disable direct access to the default key. When no named
accounts exist, the default entry remains visible.

## Use a named account in Waybar

```jsonc
"custom/openrouter-work": {
    "exec": "ai-usagebar --vendor openrouter --account work --format '{or_balance} · {or_used_today}'",
    "return-type": "json",
    "interval": 300,
    "tooltip": true
}
```

The Settings panel edits the default key. Add or change named account entries
in `config.toml`; saving other settings preserves them.
