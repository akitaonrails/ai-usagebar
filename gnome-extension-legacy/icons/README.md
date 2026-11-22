# Provider marks

Monochrome SVGs for the click menu's provider rows. Each file is named
`<brand>-symbolic.svg`, where `<brand>` is the `brand` slug `usage --json`
reports, so the extension finds a mark by name with no lookup table. The
`-symbolic` suffix makes GNOME Shell recolor it to the menu's text color, like
any symbolic icon, on light and dark themes.

The artwork is the same as `omarchy/icons/`; each frontend ships its own copy
because each installs on its own. Grok and SuperGrok share one mark, copied
under both names. A provider without a matching asset uses the system's
`application-x-executable-symbolic` icon. No list of supported vendors or
badge exceptions is needed; icons can also be hidden in preferences.

| File | Used for | Source | Licence |
|---|---|---|---|
| `anthropic-symbolic.svg` | Claude | [Simple Icons](https://github.com/simple-icons/simple-icons) `claude` | [CC0-1.0](https://creativecommons.org/publicdomain/zero/1.0/) |
| `anthropic_api-symbolic.svg` | Anthropic API | Simple Icons `anthropic` | CC0-1.0 |
| `openai-symbolic.svg` | Codex | Simple Icons `openai` | CC0-1.0 |
| `copilot-symbolic.svg` | GitHub Copilot | Simple Icons `githubcopilot` | CC0-1.0 |
| `deepseek-symbolic.svg` | DeepSeek | Simple Icons `deepseek` | CC0-1.0 |
| `kimi-symbolic.svg` | Kimi | Simple Icons `kimi` | CC0-1.0 |
| `minimax-symbolic.svg` | MiniMax | Simple Icons `minimax` | CC0-1.0 |
| `openrouter-symbolic.svg` | OpenRouter | Simple Icons `openrouter` | CC0-1.0 |
| `cursor-symbolic.svg` | Cursor | Simple Icons `cursor` | CC0-1.0 |
| `grok-symbolic.svg`, `supergrok-symbolic.svg` | Grok, SuperGrok | [lobe-icons](https://github.com/lobehub/lobe-icons) `grok` | [MIT](https://github.com/lobehub/lobe-icons/blob/master/LICENSE) |
| `grokbot-symbolic.svg` | Grok Bot | Official logomark from [x.ai/bot](https://x.ai/bot) / Grok Bot.app (head with eye cutouts) | Identification use; [xAI brand guidelines](https://x.ai/legal/brand-guidelines) |
| `zai-symbolic.svg` | Z.AI | lobe-icons `zhipu` | MIT |
| `moonshot-symbolic.svg` | Moonshot | lobe-icons `moonshot` | MIT |
| `kilo-symbolic.svg` | Kilo | lobe-icons `kilocode` | MIT |
| `novita-symbolic.svg` | Novita | lobe-icons `novita` | MIT |
| `antigravity-symbolic.svg` | Antigravity | lobe-icons `antigravity` | MIT |
| `kiro-symbolic.svg` | Kiro | lobe-icons `kiro` | MIT |
| `nous-symbolic.svg` | Nous Research | lobe-icons `nousresearch` | MIT |
| `opencode-go-symbolic.svg` | OpenCode Go | lobe-icons `opencode` | MIT |
