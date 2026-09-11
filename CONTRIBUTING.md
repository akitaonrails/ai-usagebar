# Contributing

Thanks for looking at this. Contributions land regularly and are genuinely
welcome — this file exists so review spends its time on your idea rather than
on the same handful of mechanical things.

## Before you open a PR

Run the gate. It is the same one CI runs, and it catches almost everything a
review would otherwise send back:

```
make test                                   # cargo test + the GNOME, KDE and Omarchy contract suites
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
cargo machete                               # no unused dependencies
```

`make test` rather than `cargo test`: the frontends have their own Node contract
tests, and a change to the report shape can break them without touching Rust.

## Checklist

Most review round-trips come from one of these. None takes long.

- [ ] **`CHANGELOG.md` entry**, under `## [Unreleased]`, in the right category
      (`Added` / `Changed` / `Fixed` / `Security`). Anything user-visible needs
      one — a new provider, a changed default, a fixed bug, a renamed flag.
      **Never edit a released section**; if your branch is older than the last
      tag, git will merge your entry cleanly into whatever now sits at that
      position, which has silently rewritten shipped history twice.
- [ ] **Tests that fail without your change.** For a bug fix, confirm the new
      test fails on `main` and passes on your branch — say so in the PR.
- [ ] **No leftovers.** If your change removes the last caller of a helper,
      remove the helper too. Dead shared machinery is how this codebase has
      previously grown copies that drift apart.
- [ ] **Documentation that mentions what you changed.** Grep for it: `README.md`,
      `config.example.toml`, `docs/configuration.md`,
      `docs/vendor-endpoints.md`, `docs/format-placeholders.md`. A doc that
      still describes the old behaviour is worse than one that says nothing —
      especially when it promises a security property that is no longer true.

## Project rules worth knowing

These are in `CLAUDE.md` in full. The ones contributors hit most:

- **The widget always exits 0.** Waybar hides a module that doesn't. Errors
  become a fallback `⚠` payload, never a non-zero exit.
- **No `PATH` lookups for a trusted binary** without a config override. The
  executable that runs on every refresh should not be an ambient choice — see
  `[supergrok] grok_binary` and `[copilot] gh_binary`.
- **Credentials are read, never rewritten**, unless the vendor owns the file.
  CLI, editor and browser credentials are never parsed, copied, or stored.
- **Tests are hermetic.** A `#[test]` must never read or write a real `$HOME`
  or `$XDG` path, or branch on an ambient environment variable — the AUR
  package runs `cargo test` during install, so a test coupled to your machine
  fails someone else's build. Inject the path: `Cache::at`, not `for_vendor`;
  `creds::read_from`, not `default_path`.
- **Money is formatted in one place** (`format::money` / `format::usd`), and
  a fetch outcome is built in one place (`outcome::Outcome`). Guard tests fail
  the build if a second copy appears.

## Adding a provider

The bar is: **quota reachable with a credential the user already has, obtained
the way this project obtains credentials** — an API key, an OAuth file an
official CLI wrote, or an official CLI invoked for a token. Scraped browser
sessions do not qualify. `docs/vendor-endpoints.md` records providers that have
been evaluated and why some were declined; check it before starting.

Open an issue first with the endpoint, the auth mechanism, and a real response
capture with the numbers redacted. Field names and nesting are what a parser
gets pinned to, and a paraphrase is not enough to build against.

## Platform reality

CI builds Linux, macOS and Windows, but the maintainer works on Linux. macOS
code paths — the Keychain, Claude Desktop, `safe_storage` — get compiled but
not exercised. If you are on a Mac and can test a change there, say so in the
PR; that is worth more than it sounds.
