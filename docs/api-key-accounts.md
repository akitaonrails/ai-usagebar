# API-key account guide

Every provider that reads a plain API key from its config section can report
several keys side by side, without separate config or cache roots. The
section's existing `api_key_env` / `api_key` stays the default account; each
extra key is one `[[<vendor>.accounts]]` entry.

The array works for these sections:

| Section | Provider |
|---|---|
| `[zai]` | Z.AI |
| `[openrouter]` | OpenRouter — see also the [OpenRouter account guide](openrouter-accounts.md) |
| `[deepseek]` | DeepSeek |
| `[kilo]` | Kilo |
| `[novita]` | Novita |
| `[moonshot]` | Moonshot |
| `[grok]` | Grok (xAI management key) |
| `[minimax]` | MiniMax |
| `[orcarouter]` | OrcaRouter |

Kimi is not on the list: its fallback is the Kimi Code CLI's single OAuth
login, not a second key. Claude and Codex have their own account arrays — see
the [Claude account guide](claude-accounts.md) and `[[openai.accounts]]` in
[configuration](configuration.md).

## Add named accounts

Add one entry per extra key to `~/.config/ai-usagebar/config.toml`:

```toml
[deepseek]
enabled = true
api_key_env = "DEEPSEEK_API_KEY"

[[deepseek.accounts]]
label = "work"
api_key_env = "DEEPSEEK_WORK_API_KEY"

[[deepseek.accounts]]
label = "personal"
api_key = "sk-personal"
```

An account can use `api_key_env`, an inline `api_key`, or both. The environment
variable wins when both are set. If you store any key inline, ai-usagebar
tightens the config file to mode `0600` on Unix.

Labels cannot be empty, contain path separators, drive prefixes, or control
characters, or use a reserved cache filename. Duplicate labels within one
section are rejected, and so is an entry with no key source. A label that is
not in the array is an error — it never falls back to the default key, which
would show another account's usage.

## What an entry separates, and what it shares

An entry is a key. Each one gets its own TUI tab, `usage` report entry
(`deepseek@work`), and cache directory, so one key's fresh data is never shown
for another.

Everything else in the section applies to every account of that provider:

- `region` (`[moonshot]`, `[minimax]`) — keys from two regions cannot share one
  section, because a key issued for one host is rejected by the other.
- `organization_id` (`[kilo]`) — every account reads the same organization's
  balance; leave it unset for personal balances.
- `team_id` (`[grok]`) — leave it unset so each management key resolves its
  own team; a pinned id is sent with every account's key.
- `display_limit` and `headline` — one tank size and bar number for the
  provider.

On OpenRouter, keys created inside one workspace share that workspace's
billing account; the [OpenRouter account guide](openrouter-accounts.md#what-one-entry-separates)
explains what that means for the split.

## Select an account

Named accounts appear automatically as separate TUI tabs, `usage` report
entries, and entries in the native integrations. Select one directly in the
widget:

```bash
ai-usagebar --vendor deepseek --account work
```

The default account keeps the original `~/.cache/ai-usagebar/<vendor>/` cache.
Named accounts use `~/.cache/ai-usagebar/<vendor>/<label>/`.

To omit the unnamed account from aggregate views when all keys are named:

```toml
[deepseek]
show_default_account = false
```

This setting does not disable direct access to the default key. When no named
accounts exist, the default entry remains visible.

`ai-usagebar detect` counts a named key as a credential, so a provider whose
keys are all named is still switched on.

## Use a named account in Waybar

```jsonc
"custom/deepseek-work": {
    "exec": "ai-usagebar --vendor deepseek --account work",
    "return-type": "json",
    "interval": 300,
    "tooltip": true
}
```

The Settings panel edits the default key. Add or change named account entries
in `config.toml`; saving other settings preserves them.
