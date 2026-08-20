# GitHub sync setup and authentication

The sync feature backs up your usage history and configuration to a private GitHub
repository, encrypted with a password only you hold. This guide covers repository
creation, token setup, what the tool checks before it will touch your data, and the
four commands: `sync setup`, `sync push`, `sync prune`, and `sync rekey`.

`docs/sync-format.md` is the specification — the on-the-wire layout, the crypto, and
the honest limits. This page is the one you read to use the thing; where it needs a
detail, it links there rather than repeating it.

## Create your backup repository

**The tool never creates a repository, and it cannot.** The token you give it is configured without `Administration: write`, so repository creation is not a permission it holds — it is structurally impossible for the tool to create a repository, public or private, even by accident. You create the repository yourself, **private**, and you create a fine-grained personal access token with exactly two permissions: `Contents: read/write` and `Metadata: read`. Naming the repository is a one-time, explicit act you perform once.

You can create the repository either way:

**Via GitHub CLI (recommended):**

```bash
gh repo create <owner>/<name> --private
```

Replace `<owner>` with your GitHub username or organization, and `<name>` with your chosen repository name (e.g. `gh repo create alice/ai-usagebar-backup --private`).

**Via GitHub web form:**

1. Go to https://github.com/new
2. Name your repository
3. Select **Private** visibility
4. Leave other options at their defaults
5. Click **Create repository**

Tell ai-usagebar the repository's location by adding it to `~/.config/ai-usagebar/config.toml`:

```toml
[sync]
repo = "owner/name"
```

A missing or unset `repo` value is an actionable error that prints the exact command to create one. The tool will not guess a repository name, so a missing repo always means you need to create one first.

## Create a fine-grained personal access token

The token must be scoped to your single backup repository with exactly two permissions. This prevents a leaked token from being a skeleton key across everything you own.

**On GitHub:**

1. Go to **Settings → Developer settings → Personal access tokens → Fine-grained tokens**
2. Click **Generate new token**
3. **Name:** something descriptive (e.g. `ai-usagebar-sync`)
4. **Expiration:** choose a duration (e.g. 90 days)
5. **Repository access:** Select **Only select repositories**, then choose your backup repository from the list
6. **Repository permissions:**
   - **Contents:** Check **Read and write**
   - **Metadata:** **Read-only** (checked by default, cannot be removed)
7. **Do NOT grant Administration permissions.** This field must remain unchecked. The lack of this permission is what structurally prevents the token from creating or modifying repository settings — it is the whole enforcement, and the unchecked box is the control. There is deliberately **no warning** if you grant it anyway: `GET /repos/{owner}/{repo}` reports the authenticated *user's* role on the repository rather than the token's granted permissions, and since you create the repository yourself you are always its admin — so the warning would fire on every legitimate install, and a warning that always fires trains you to ignore it. Whether a fine-grained token narrows that field is an open question tracked in `docs/sync-format.md` §7; if it turns out to report the grant, the warning goes back in.
8. Click **Generate token** and copy the token value immediately (GitHub only displays it once)

Your token carries exactly two permissions:
- **Contents: read/write** — needed to push and pull your data
- **Metadata: read** — needed to verify the repository's visibility and settings

Nothing else.

## Where the token is stored

The tool looks for the token in this order:

1. **`AI_USAGEBAR_SYNC_TOKEN` environment variable** — Use this for CI environments and for headless restores over SSH. No persistent storage, no file-system reads. This is the override.

2. **macOS Keychain item** (macOS only) — Service name `ai-usagebar-sync-token`. The tool reads it via `security(1)` and writes via Security.framework, so the token never appears in process arguments or shell history.

3. **`sync-token` file beside `config.toml`** — normally `~/.config/ai-usagebar/sync-token`, mode 0600 (read/write by owner only), matching the existing credential-file convention. This is where `sync setup` *writes* the token when you are not on macOS. It is *read* on every platform, so a token file copied from another machine still works on a Mac.

4. **`gh auth token`** (convenience fallback) — If the GitHub CLI is installed and logged in, the tool can use its authentication. This is never required; it is a convenience so you do not need to manage a separate token.

If GitHub rejects the token with a 401, the tool clears **only the store the rejected token came from** — the Keychain item if it came from the Keychain, the `sync-token` file if it came from the file, and nothing at all if it came from `AI_USAGEBAR_SYNC_TOKEN` or from `gh`. Those two are not the tool's to delete, and a 401 on one of them says nothing about a stored token that was never sent. The message names which of them to change instead: while the environment variable is set, a replacement written anywhere else is never reached.

The tool never writes the token into `config.toml`. Inline API keys there are a deliberate choice for read-only provider keys (Claude, OpenAI, Z.AI) because a leaked key is a minor risk on a read-only endpoint. A token that can write to a repository is a different class of secret, so it lives in a separate, mode-0600 location (or in the Keychain on macOS).

### Why not D-Bus secret service?

The `keyring` and `secret-service` packages provide a unified secrets interface via D-Bus. They require a live D-Bus session and an unlocked keyring, which fails in headless environments and over SSH — exactly the scenario this sync feature exists to serve. The project already has platform-specific credential handling (Keychain on macOS, local files on Linux), so those are reused here instead of adding a new runtime dependency.

## Repository verification

Before any sync operation, the tool verifies the repository meets these conditions:

- **Private visibility** — the repository is marked private, not internal or public
- **Owned by the configured user** — the repository's owner numeric ID matches the one recorded at first pairing
- **Not archived** — archived repositories reject pushes
- **Not a fork** — a fork shares its upstream's object network, so objects pushed to it can be reachable from the public parent

This check runs **immediately before every push**, not once at setup. A repository can be made public from the web interface at any moment, so the tool re-checks every time.

### If the repository is found public

If the tool detects the repository has become public after previously being private:

1. The operation aborts and prints an error
2. You are told to make the repository private again
3. **You must rotate any credentials that may already have been pushed**, because a previous push may have succeeded while the repository was public, and bytes that have been published cannot be un-published

This is not optional. If you backed up credentials before you made the repository public, those credentials are compromised.

## What `sync setup` does and does not do

`sync setup` authenticates, resolves the repository, and verifies it is private. It performs **zero uploads** — pairing and pushing are separate commands, so verifying the pairing never costs bandwidth.

It is a guided, five-step flow:

1. **The categories.** Shows what gets bundled and lets you toggle each one, `credentials` included and explicit. This comes first because it is an *input to the gate*: whether `credentials` is in the bundle is what decides whether a public repository is refused outright or merely warned about, and the gate has to be asked the question you actually answered.
2. **The repository and the gate.** Resolves the token, fetches the repository's details, and refuses unless it passes every condition above, judged against the categories you just chose. Every refusal stops *here* — you are never asked to choose a sync password for a repository that is about to be rejected. On a first pairing it names the repository and owner ids it is pairing with; if this machine was already paired, that line means the pairing record went missing, which is worth investigating.
3. **The sync password.** Offers a generated 20-character passphrase (press Enter to take it) or accepts your own, subject to a length floor. There is no recovery: the password is the only thing that can open the bundle. If a keyfile is already at `<config dir>/sync/keyfile.json`, setup stops before anything else — overwriting one makes every bundle written under the old password permanently unreadable.
4. **The size.** Runs the same planner `sync push --dry-run` runs and shows its figures — files, raw bytes, and what a first push would actually send — then asks you to confirm.
5. **Everything that persists.** Writes the keyfile at mode 0600, saves your category choices back into `config.toml` with comments and key order preserved, stores the token where only you can read it (the Keychain on macOS, the mode-0600 file elsewhere), and records the pairing. Nothing before this point writes anything, so declining at step 4 leaves the machine exactly as it was and the command can simply be re-run.

## What `sync push` does

```bash
ai-usagebar sync push
```

It re-checks that the repository is private **before the first byte and again before publishing** — every push, never once at setup. A repository can be made public from the web interface between the two, and the second check is what that is for. Then it uploads the encrypted data as release assets, downloads each one back and checks it matches what was sent, and only then publishes the snapshot pointer with a compare-and-swap precondition.

That last `PUT` is the only step that changes what a reader sees. Everything above it is inert: the assets are content-addressed and nothing references them yet. **Interrupting a push at any point before the flip leaves the previous snapshot exactly as it was**, byte for byte.

Things worth knowing about the shape of a push:

- **It uploads several assets, not one.** Your files are packed into large objects, and the bundle's own manifest and index travel in packs alongside them — so even a one-file bundle produces more than one. A separate small asset carries your wrapped master key; it is published after the second privacy check, not before, because it is the most sensitive object in the bundle.
- **It is a handful of requests, not one per file.** A push of ~190 chunks costs 13 HTTP requests in total: nine fixed, plus one upload and one verifying download per pack. The very first push against a repository costs one more, because the release has to be created before anything can hang off it. The count tracks packs, never your data.
- **`--dry-run` shows what a push would send** without sending it, and needs no network.
- **It refuses a bundle that went backwards.** Each machine remembers the highest snapshot counter it has seen published in this repository. A remote offering a *lower* one is an older snapshot replayed to hide a newer one — the one kind of tampering that authenticates perfectly, because the old data really was written by your key. The push stops before anything is packed. If you know why the remote went back — you rebuilt the bundle from scratch, or restored an older one on purpose — `ai-usagebar sync push --allow-rollback` is the way through. `sync prune` and `sync rekey` refuse the same input and offer no override; neither is a command you reach for when you mean to move the bundle backwards.
- **It refuses if another machine changed the password and this one has not caught up.** See below.

### Re-running an interrupted push

Re-running is safe and is the intended recovery. A push lists what is already on the release and skips anything that matches by name, size *and* upload state — all three, because GitHub creates an asset record before the body finishes, so a torn upload carries the right name. A torn asset is deleted and re-sent rather than trusted.

Asset names are content addresses, and the bundle's manifest — which lists every file in the bundle — travels inside a pack, so anything that changes the manifest's bytes changes those addresses. The manifest lists files in path order for exactly that reason: the addresses depend on what is on disk, not on what your local index happens to have seen before, and not on the order your filesystem enumerated the directory. So a re-run after an interrupted push re-sends nothing.

### What progress looks like

Progress goes to **standard error**, at asset granularity — "uploading 2/3 assets — 24.0 MiB of 36.0 MiB". On a terminal it rewrites one line in place. Anywhere else — a pipe, a log file, the macOS menu bar capturing the command as a subprocess — it degrades to one plain line per completed asset, with no carriage returns and no escape sequences.

**Standard output carries the result**, so piping the command stays clean:

```
uploaded:   2 pack(s), 24.0 MiB
skipped:    3 pack(s) already present
snapshots:  4 kept
pruned:     1 pack(s)

The snapshot is published.
```

## Retention, and what a prune deletes

The remote keeps the last **10 snapshots** by default. Change it with:

```toml
[sync]
keep_snapshots = 10
```

Old snapshots are cheap: they share packs with newer ones, so keeping ten costs little more than keeping three. `0` is refused when the config is loaded, and `prune` clamps it to at least one — a zero would mean the flip that publishes a snapshot also drops it.

**A prune runs automatically after every successful push.** It drops snapshot records past `keep_snapshots`, oldest first, and then deletes pack assets no surviving snapshot still references. The record always goes first: the reverse order can leave a live snapshot pointing at a pack that is gone, which is an unrestorable backup and the worst thing this feature could produce.

**A prune failure is a warning, never a failed push.** If the cleanup step fails you will see:

```
warning:    the push succeeded and the snapshot is published, but cleaning up
            superseded data did not: <reason>
            This costs storage, not correctness. `ai-usagebar sync prune` retries it.
```

Read that literally. Your data is safe and the snapshot is published; some storage was not reclaimed. Run the on-demand form when convenient:

```bash
ai-usagebar sync prune
```

### The two guards are not interchangeable

A prune is the one destructive operation in the tool, and it is protected by two rules. **Neither is sufficient alone, and neither replaces the other:**

- The deletion set is computed against the pointer that **landed** — whatever the remote returned from the compare-and-swap, never the one this machine built. That closes the *committed* competitor: if another machine won the race, the landed pointer is its pointer and its packs are live.
- **Nothing younger than 24 hours is deleted, whatever the pointer says.** That closes the *in-flight* competitor, which a landed pointer cannot see by definition: a machine that has uploaded packs and has not flipped yet is referenced by no snapshot at all. The cost is that genuine garbage lingers for a day.

So a `sync prune` immediately after a push will not reclaim data that only just became superseded. That is the guard doing its job, not a failure. `docs/sync-format.md` §10 has the full rule.

One case where an on-demand prune deletes nothing at all: if this bundle has **never published a snapshot pointer**, `sync prune` returns having done nothing. Without a pointer there is nothing to prove an asset is garbage against, and creating a release purely to run a delete would be wrong. A stray asset left by an interrupted first push therefore stays until a push succeeds; after that, the ordinary prune collects it.

## Changing the sync password

```bash
ai-usagebar sync rekey
```

It asks for the current password, then the new one — on the terminal, or on standard input; never as a command-line argument and never through an environment variable. It unwraps the master key with the old password, rewraps *the same* master key with the new one, publishes the new keyfile, points the bundle at it, and then **deletes the old keyfile asset and re-lists to confirm it is really gone**. A delete that cannot be confirmed is reported as a failure, not a warning: this command's entire value is that the old wrapper is destroyed.

Not one pack byte moves. That is the point — a password change costs 48 rewritten bytes instead of re-uploading the whole bundle.

**A rekey is not revocation.** The command says so on the way in and on the way out, and it means it:

> Changing the sync password rewraps the master key. Not one pack byte moves — and this is NOT revocation: anyone who already holds a copy of the old keyfile can still open it with the old password.

Anyone holding a copy of the old keyfile can still open the bundle with the old password — **including data written after the change**, because the data keys never changed. Deleting the remote asset removes the copy this tool published; it cannot reach one somebody already took. Real revocation means a new master key and re-encrypting the entire bundle, which is exactly the whole-bundle re-upload this command exists to avoid. See `docs/sync-format.md` §9.

If this bundle has never published a pointer, `sync rekey` changes the password locally and uploads nothing — there is no remote wrapper to replace yet. The next successful push publishes the new keyfile through the ordinary path.

### After a rekey, catch up your other machines

A rekey changes the password for the *bundle*, and the machine that ran it. Every other machine still holds the superseded keyfile on disk, and that file is exactly what the rekey destroyed remotely.

So a push from a machine that missed the rekey **refuses**, before it sends a byte, and tells you so. It will not publish its own wrapper over the current one: doing that would put the old wrapper back on the remote, where the old password opens it again — and the password change would have been cosmetic.

Nothing is wrong with that machine's data. A rekey rewraps the master key and re-encrypts nothing, so every byte it has already pushed is still readable under the new password. To catch it up, copy the keyfile from the machine where you changed the password — it sits next to `config.toml`, at `sync/keyfile.json` — onto the stale machine, replacing the local file. Then re-run the push.

Check the status of your pairing with:

```bash
ai-usagebar sync status
```

It prints the category listing, then the repository:

```
  repo:      owner/name
  visible:   private
  token:     present (env)
  verified:  2026-08-19T12:00:00+00:00
```

The `token:` line reports the token's **source**, never its value. The four labels are `env`, `Keychain`, `file`, and `gh`, matching the resolution order above. `verified:` is when this machine last confirmed the pairing.

The repository half and the category listing fail independently: an expired token still leaves the listing visible, so you can always see what *would* be sent — but any repository-section failure is a non-zero exit.

## GitHub's acceptable-use policy

There is **no prohibition** on using a private repository for backups, and nothing here is a warning that you are doing something wrong. But GitHub's acceptable-use policy reserves the right to throttle or suspend accounts whose bandwidth or storage use is significantly out of line with comparable users, and the profile that draws attention is a specific one: a **multi-gigabyte bundle rewritten frequently**. It is worth knowing the shape of that rather than discovering it by surprise, because your account is not a storage tier and should not be turned into one without your knowing.

Two mitigations are already built in and need no configuration:

- **Content-addressed packs.** An unchanged file is never re-uploaded — a push only sends what actually changed since the last one. `sync push --dry-run` shows that number before you spend it.
- **Prune.** Superseded generations are deleted automatically after every successful push, so remote size tracks live data rather than cumulative history. In a twelve-push measurement of a growing file at `keep_snapshots = 3`, 25 assets were uploaded over the run and 14 remained.

If you plan to push very frequently, `sync push --dry-run` and your GitHub account's storage page are the two numbers to watch.
