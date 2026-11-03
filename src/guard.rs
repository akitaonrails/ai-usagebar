//! Source-scanning helpers for structural guard tests.
//!
//! A guard that enumerates what to check fails open on everything added after
//! it. A guard that *walks* fails closed: a new file is scanned by default and
//! an exemption has to be written down. `claude_desktop`'s note-sanitization
//! guard is built on this.
//!
//! Written by Augusto Claro for the encrypted-sync bundle format (#123) and
//! kept when that module was reverted, because the walking-guard idea outlived
//! the feature it was written for. The `production_code` comment below records
//! a real defect it was hardened against; the file names in it refer to that
//! now-removed module and are left as the history of the fix.

use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`, recursively.
pub(crate) fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

/// Every `.rs` file under `CARGO_MANIFEST_DIR`-relative `rel`.
///
/// Resolved from the manifest directory rather than from a relative path, so
/// a guard is independent of the working directory and survives the AUR
/// `srcdir` layout.
pub(crate) fn rs_files_in(rel: &str) -> Vec<PathBuf> {
    rs_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join(rel))
}

/// A file's production code: every line that is neither a comment nor part
/// of the file's own `#[cfg(test)]` module. A test that names a needle is a
/// test, not a violation, and prose that discusses one is neither.
///
/// **Comments are removed before the marker is looked for, and that is the
/// fix.** The previous shape split the raw source on the first *textual*
/// `#[cfg(test)]`. In `github/pairing.rs` the first occurrence is inside a
/// doc comment at line 76, so the scanned region ended at line 75 and the
/// 397 lines below it — five production functions — were invisible to every
/// guard built on this helper. Phase 5's audit put
/// `std::env::var("SYNC_PASSWORD")` in that region and watched the T-5-66
/// guard pass.
///
/// `github/mod.rs`'s own guard recorded this exact defect and worked around
/// it for itself; the lesson reached one call site and not the shared helper
/// every other guard depends on. A *smarter* marker search — line-anchored,
/// or `\n#[cfg(test)]\nmod tests` — keeps the same shape: a guard that stops
/// looking where it happens to find a string. Dropping comments first makes
/// the marker unambiguous by construction, because prose is no longer part
/// of the text being searched.
///
/// Returns an owned `String` rather than a borrowed slice, since the result
/// is no longer a contiguous piece of the input.
pub(crate) fn production_code(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .take_while(|line| !line.trim_start().starts_with("#[cfg(test)]"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every call of `opener` (a macro or method head ending in `(`, such as
/// `"println!("`) in `code`, as the text from the opener through its matching
/// `)`. A call left unclosed runs to the end of `code`.
///
/// Parentheses inside string and character literals are not counted: a message
/// like `"step 1) done"` must not end the call early and hide an argument
/// after it, and a stray `)` must not underflow the depth. Matching is
/// textual, so `"println!("` also finds the tail of `eprintln!(` — pass
/// `"println!("` and `"print!("` to cover all four macros without visiting one
/// twice.
pub(crate) fn calls<'a>(code: &'a str, opener: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = code[from..].find(opener) {
        let start = from + found;
        let end = start + call_len(&code[start..]);
        out.push(&code[start..end]);
        from = end.max(start + 1);
    }
    out
}

/// Byte length of the balanced call at the start of `call`. Every byte the
/// scanner branches on is ASCII, so each slice boundary is a char boundary.
fn call_len(call: &str) -> usize {
    let bytes = call.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1;
                }
            }
            b'"' => i = string_end(bytes, i, None),
            b'r' => {
                if let Some(hashes) = raw_string_hashes(bytes, i) {
                    i = string_end(bytes, i + 1 + hashes, Some(hashes));
                }
            }
            b'\'' => {
                // `'('` and `'\''` are char literals; `'a` is a lifetime.
                if bytes.get(i + 1) == Some(&b'\\') {
                    i = bytes[i + 2..]
                        .iter()
                        .position(|&b| b == b'\'')
                        .map_or(bytes.len(), |n| i + 2 + n);
                } else if bytes.get(i + 2) == Some(&b'\'') {
                    i += 2;
                }
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// Index of the last byte of the string whose opening quote is at `open`.
/// `raw` carries the `#` count of a raw string, which ignores escapes and
/// closes on a quote followed by that many hashes; `None` is an ordinary
/// string, which skips `\"`.
fn string_end(bytes: &[u8], open: usize, raw: Option<usize>) -> usize {
    let hashes = raw.unwrap_or(0);
    let mut i = open + 1;
    while i < bytes.len() {
        if raw.is_none() && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'"' && bytes[i + 1..].iter().take(hashes).all(|&b| b == b'#') {
            return i + hashes;
        }
        i += 1;
    }
    bytes.len()
}

/// The `#` count of a raw string literal starting at `at` (`r"…"`, `r#"…"#`),
/// or `None` when `at` is just an `r` inside an identifier.
fn raw_string_hashes(bytes: &[u8], at: usize) -> Option<usize> {
    if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_') {
        return None;
    }
    let hashes = bytes[at + 1..].iter().take_while(|&&b| b == b'#').count();
    (bytes.get(at + 1 + hashes) == Some(&b'"')).then_some(hashes)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a readable source directory") {
        let path = entry.expect("a readable directory entry").path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::vendor::VendorId;
    use std::collections::BTreeMap;

    /// One released section's entries must not reappear anywhere else.
    ///
    /// A PR branched before the last tag keeps its notes under `[Unreleased]`,
    /// and git merges them *cleanly* into whatever now occupies that position
    /// — which is the section that was just published. The repository has been
    /// bitten three times: twice by contributor branches, and once by a
    /// maintainer resolving a conflict with a script, which is what turned a
    /// documented manual check into this test.
    ///
    /// The manual check compares the newest section against its own tag and so
    /// needs git, which the AUR `check()` does not have. This needs only the
    /// file: an entry duplicated across two versions is the shape the accident
    /// takes, whichever direction it came from.
    #[test]
    fn no_changelog_entry_appears_under_two_versions() {
        let Some(changelog) = changelog() else { return };
        let mut seen: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut version = "<before any version heading>";

        for line in changelog.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                version = rest.trim();
            } else if let Some(entry) = line.strip_prefix("- ") {
                let entry = entry.trim();
                // The first line of a bullet is its identity; continuation
                // lines are indented and never start a bullet.
                if entry.len() > 24 {
                    seen.entry(entry).or_default().push(version);
                }
            }
        }

        let duplicated: Vec<_> = seen
            .iter()
            .filter(|(_, versions)| {
                let mut distinct = versions.to_vec();
                distinct.sort_unstable();
                distinct.dedup();
                distinct.len() > 1
            })
            .map(|(entry, versions)| format!("{versions:?}: {}…", &entry[..40.min(entry.len())]))
            .collect();

        assert!(
            duplicated.is_empty(),
            "these entries appear under more than one version — an [Unreleased] \\
             note has merged into a released section:\n{}",
            duplicated.join("\n")
        );
    }

    /// Keep-A-Changelog allows one heading per category per release, and the
    /// release process reads those headings to pick the next version number. A
    /// second `### Fixed` in one section hides whatever is under it from that
    /// decision. Merging two branches that both add a category produces
    /// exactly this.
    #[test]
    fn no_changelog_section_repeats_a_category() {
        let Some(changelog) = changelog() else { return };
        let mut offenders = Vec::new();
        let mut version = "<before any version heading>";
        let mut categories: Vec<&str> = Vec::new();

        let mut flush = |version: &str, categories: &mut Vec<&str>| {
            let mut sorted = categories.clone();
            sorted.sort_unstable();
            let before = sorted.len();
            sorted.dedup();
            if sorted.len() != before {
                offenders.push(version.to_string());
            }
            categories.clear();
        };

        for line in changelog.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                flush(version, &mut categories);
                version = rest.trim();
            } else if let Some(rest) = line.strip_prefix("### ") {
                categories.push(rest.trim());
            }
        }
        flush(version, &mut categories);

        // Two released sections shipped this way. Their text is frozen — a
        // tag is immutable and the drift rule forbids editing a published
        // section — so they are named here rather than corrected. The point of
        // this guard is the next one, not the last two.
        const SHIPPED_THIS_WAY: [&str; 2] = ["[1.8.0] — 2026-08-28", "[0.14.0] — 2026-07-20"];
        offenders.retain(|version| !SHIPPED_THIS_WAY.contains(&version.as_str()));

        assert!(
            offenders.is_empty(),
            "these versions carry the same category heading twice, which hides \
             whatever is under the second one from the release's version \
             decision: {offenders:?}"
        );
    }

    /// The changelog, when this build has one.
    ///
    /// `nix/package.nix` filters the source down to what the binary needs, and
    /// `CHANGELOG.md` is not in it — correctly, since adding it would rebuild
    /// the package every time a release note changes. These guards protect the
    /// repository's changelog, so they have nothing to say in a build that
    /// ships without one and skip rather than fail.
    ///
    /// They still run everywhere it matters: a developer's `make test`, the
    /// Linux/macOS/Windows CI jobs, and the AUR `check()`, whose source is the
    /// release tarball and does include the file.
    fn changelog() -> Option<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("CHANGELOG.md");
        std::fs::read_to_string(&path).ok()
    }

    /// How the macOS menu bar mirrors one vendor's figures.
    ///
    /// Two cases, because the mirror is a deliberate *subset*: Rust emits 289
    /// placeholders and the Swift `FORMAT` asks for 57 of them. Which figures
    /// the bar shows is a product decision, not a mechanical fact, so it stays
    /// written down — what is no longer optional is *making* the decision.
    #[derive(Debug)]
    enum MacosMirror {
        /// The bar asks for this vendor's own placeholder. The token must be in
        /// the Swift `FORMAT` and must still be emitted by Rust, so a rename on
        /// either side fails instead of silently dropping the vendor's row.
        Slot(&'static str),
        /// The bar renders this vendor through these generic placeholders, so it
        /// has no slot of its own. The tokens are listed rather than implied:
        /// claiming "generic" has to be *true*, or it becomes a rubber stamp
        /// that silences a vendor the bar actually cannot show.
        Generic(&'static [&'static str]),
    }

    /// Exhaustive on purpose, and that is the entire fix: a new `VendorId` does
    /// not compile until someone classifies it.
    ///
    /// What this replaces was a hand-written list of nineteen tokens. It could
    /// only catch a token that was *listed* and missing from the mirror — a
    /// vendor missing from both the list and the mirror passed in silence,
    /// which is exactly how three vendors in a row shipped without their slot
    /// (#372 was the third). Nothing tied that list to the set of vendors that
    /// exist, so it could not close the hole it was written to close.
    fn macos_mirror(id: VendorId) -> MacosMirror {
        // The generic core every vendor without its own slot renders through.
        const CORE: &[&str] = &["plan", "vendor_short", "session_pct", "weekly_pct"];
        match id {
            // Vendors with a slot of their own in the Swift FORMAT.
            VendorId::AnthropicApi => MacosMirror::Slot("aapi_headline"),
            VendorId::Antigravity => MacosMirror::Slot("scoped_model"),
            VendorId::CommandCode => MacosMirror::Slot("cc_monthly_pct"),
            VendorId::Copilot => MacosMirror::Slot("copilot_completions_pct"),
            VendorId::Cursor => MacosMirror::Slot("cursor_total_pct"),
            VendorId::Deepinfra => MacosMirror::Slot("dif_balance"),
            VendorId::Deepseek => MacosMirror::Slot("ds_balance"),
            VendorId::Devin => MacosMirror::Slot("devin_daily_pct"),
            // Standard's 5-hour/weekly windows ride the generic slots.
            VendorId::Droid => MacosMirror::Generic(CORE),
            VendorId::MuseCode => MacosMirror::Generic(CORE),
            VendorId::Grok => MacosMirror::Slot("grok_balance"),
            VendorId::Kilo => MacosMirror::Slot("kilo_balance"),
            VendorId::Lyceum => MacosMirror::Slot("lyceum_balance"),
            VendorId::Minimax => MacosMirror::Slot("minimax_video_pct"),
            VendorId::Moonshot => MacosMirror::Slot("km_balance"),
            VendorId::Novita => MacosMirror::Slot("nv_balance"),
            VendorId::Synthetic => MacosMirror::Slot("synthetic_pct"),
            VendorId::Ollama => MacosMirror::Slot("oll_monthly_pct"),
            VendorId::OpenCodeGo => MacosMirror::Slot("ocg_monthly_pct"),
            VendorId::Openrouter => MacosMirror::Slot("or_balance"),
            VendorId::Supergrok => MacosMirror::Slot("sgk_period"),
            VendorId::Zai => MacosMirror::Slot("zai_mcp_pct"),
            // Vendors whose shape is a plain session and/or weekly window, which
            // the bar already renders from the generic placeholders. Verified:
            // each of these emits the core tokens listed here.
            VendorId::Anthropic => MacosMirror::Generic(CORE),
            VendorId::Openai => MacosMirror::Generic(CORE),
            VendorId::Kimi => MacosMirror::Generic(CORE),
            VendorId::Kiro => MacosMirror::Generic(CORE),
            VendorId::NousResearch => MacosMirror::Generic(CORE),
            VendorId::OrcaRouter => MacosMirror::Generic(CORE),
            // Weekly-only: no 5h window, so it renders through the weekly half.
            VendorId::Grokbot => MacosMirror::Generic(&["plan", "vendor_short", "weekly_pct"]),
            // Three independent windows (5 h, weekly, monthly); the macOS bar
            // reads its fields by index, so each gets its own appended slot.
            VendorId::Cline => MacosMirror::Slot("cline_5h_pct"),
            VendorId::ModelStudio => MacosMirror::Slot("mst_monthly_pct"),
            // Monthly allowance + rolling weekly caps, rendered through the
            // generic slots: `session_pct` carries the monthly window and
            // `weekly_pct` the rolling cap. DeepRelay's own Swift dispatch case
            // only relabels them ("Monthly" / "Weekly").
            VendorId::DeepRelay => MacosMirror::Generic(CORE),
        }
    }

    /// Every placeholder key Rust actually hands to `format::placeholders`.
    ///
    /// Used to fail a token that no longer exists on the Rust side: the old
    /// guard asserted only that the Swift `FORMAT` contained a literal, so a
    /// Rust-side rename left both the list and the mirror agreeing about a
    /// placeholder nothing produced any more.
    fn rust_placeholder_keys() -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        let mut files = super::rs_files_in("src");
        files.sort();
        for file in files {
            let Ok(source) = std::fs::read_to_string(&file) else {
                continue;
            };
            let code = super::production_code(&source);
            // `("key",` and `.insert("key"` are the two shapes every vendor uses.
            for (idx, _) in code.match_indices('"') {
                let rest = &code[idx + 1..];
                let Some(close) = rest.find('"') else { break };
                let key = &rest[..close];
                if key.is_empty()
                    || !key
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                {
                    continue;
                }
                let before = code[..idx].trim_end();
                let after = rest[close + 1..].trim_start();
                let opens_tuple = before.ends_with('(') && after.starts_with(',');
                let is_insert = before.ends_with(".insert(");
                if opens_tuple || is_insert {
                    out.insert(key.to_string());
                }
            }
        }
        out
    }

    /// The macOS menu bar parses a flat `FORMAT` string by field index, so a
    /// vendor whose figures are missing from it cannot be shown at all. Three
    /// vendors shipped that way before this guard existed (#372 was the third).
    ///
    /// The contract is three-sided, and each side catches a different mistake:
    /// the classification is exhaustive over `VendorId` (so a new vendor cannot
    /// be forgotten), every claimed token must be in the Swift `FORMAT` (so the
    /// mirror cannot lag), and every claimed token must still be emitted by
    /// Rust (so a rename cannot leave the two sides agreeing about nothing).
    #[test]
    fn macos_format_mirrors_every_vendor_placeholder_family() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("macos/ai-usagebar-menubar.swift");
        let Ok(swift) = std::fs::read_to_string(&path) else {
            return;
        };
        let format = match swift.split_once("let FORMAT = ") {
            Some((_, rest)) => rest.split("__aiub_end__").next().unwrap_or(rest),
            None => panic!("macOS menubar FORMAT string not found"),
        };
        let emitted = rust_placeholder_keys();
        assert!(
            emitted.len() > 100,
            "the placeholder scan found only {} keys — the scan broke, not the mirror",
            emitted.len()
        );

        for &id in VendorId::all() {
            let tokens: Vec<&str> = match macos_mirror(id) {
                MacosMirror::Slot(token) => vec![token],
                MacosMirror::Generic(tokens) => {
                    assert!(
                        !tokens.is_empty(),
                        "{id:?} claims the generic placeholders but names none — \
                         list the tokens it renders through, or give it a slot"
                    );
                    tokens.to_vec()
                }
            };
            for token in tokens {
                assert!(
                    format.contains(&format!("{{{token}}}")),
                    "macOS FORMAT is missing {{{token}}} for {id:?} — append its slot \
                     (never insert: the Swift side reads fields by index)"
                );
                assert!(
                    emitted.contains(token),
                    "{id:?} is mirrored through {{{token}}}, but no Rust code emits that \
                     placeholder any more — it was renamed on one side only"
                );
            }
        }
    }

    /// Every `;;`-separated FORMAT field is exactly one `{token}` — a single-`;`
    /// joint merges two fields and shifts every index after it (#411 nearly
    /// shipped a Devin regression that the contains-check above cannot see).
    #[test]
    fn macos_format_fields_are_single_tokens_between_separators() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("macos/ai-usagebar-menubar.swift");
        let Ok(swift) = std::fs::read_to_string(&path) else {
            return;
        };
        let expr = swift
            .split_once("let FORMAT = ")
            .and_then(|(_, rest)| rest.split_once("let FORMAT_WITH_SENTINEL"))
            .map(|(expr, _)| expr)
            .expect("macOS menubar FORMAT string not found");
        // Collapse the literal concatenation: quotes, ` +` joints, whitespace.
        let flat: String = expr
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '"' && *c != '+')
            .collect();
        let flat = flat.strip_suffix(';').map(str::to_string).unwrap_or(flat);
        for field in flat.split(";;") {
            assert!(
                field.starts_with('{')
                    && field.ends_with('}')
                    && field[1..field.len() - 1]
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "macOS FORMAT field is not exactly one placeholder: {field:?}"
            );
        }
    }

    /// Every AUR source array must have a matching sha256sums array of the same
    /// length, both in PKGBUILDs and .SRCINFO files (#335).
    ///
    /// When sources and checksums drift (e.g. adding a detached .sig without a
    /// matching 'SKIP'), makepkg rejects the package.
    #[test]
    fn aur_checksum_arrays_match_source_arrays() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let aur_dir = root.join("packaging/aur");
        if !aur_dir.exists() {
            return;
        }

        for filename in ["PKGBUILD", "PKGBUILD-bin"] {
            let path = aur_dir.join(filename);
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let sources = count_pkgbuild_arrays(&content, "source");
            let checksums = count_pkgbuild_arrays(&content, "sha256sums");
            assert!(
                !sources.is_empty(),
                "{filename} must declare at least one source array"
            );
            for (src_var, src_len) in &sources {
                let suffix = src_var.strip_prefix("source").unwrap_or("");
                let sha_var = format!("sha256sums{suffix}");
                let sha_len = checksums.get(&sha_var).copied().unwrap_or(0);
                assert_eq!(
                    *src_len, sha_len,
                    "{filename}: {src_var} ({src_len} elements) and \
                     {sha_var} ({sha_len} elements) differ in length"
                );
            }
            for sha_var in checksums.keys() {
                let suffix = sha_var.strip_prefix("sha256sums").unwrap_or("");
                let src_var = format!("source{suffix}");
                assert!(
                    sources.contains_key(&src_var),
                    "{filename}: {sha_var} declared without corresponding {src_var}"
                );
            }
        }

        for filename in [".SRCINFO", ".SRCINFO-bin"] {
            let path = aur_dir.join(filename);
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let sources = count_srcinfo_lines(&content, "source");
            let checksums = count_srcinfo_lines(&content, "sha256sums");
            assert!(
                !sources.is_empty(),
                "{filename} must declare at least one source entry"
            );
            for (src_key, src_len) in &sources {
                let suffix = src_key.strip_prefix("source").unwrap_or("");
                let sha_key = format!("sha256sums{suffix}");
                let sha_len = checksums.get(&sha_key).copied().unwrap_or(0);
                assert_eq!(
                    *src_len, sha_len,
                    "{filename}: {src_key} ({src_len} lines) and \
                     {sha_key} ({sha_len} lines) differ in count"
                );
            }
            for sha_key in checksums.keys() {
                let suffix = sha_key.strip_prefix("sha256sums").unwrap_or("");
                let src_key = format!("source{suffix}");
                assert!(
                    sources.contains_key(&src_key),
                    "{filename}: {sha_key} declared without corresponding {src_key}"
                );
            }
        }
    }

    fn count_pkgbuild_arrays(content: &str, prefix: &str) -> BTreeMap<String, usize> {
        let mut results = BTreeMap::new();
        let mut in_array: Option<(String, usize)> = None;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            if let Some((name, count)) = in_array.as_mut() {
                let part = trimmed.split('#').next().unwrap_or("").trim();
                *count += count_quoted_items(part);
                if part.contains(')') {
                    results.insert(name.clone(), *count);
                    in_array = None;
                }
            } else if let Some((var_name, rest)) = trimmed.split_once("=(") {
                let var_name = var_name.trim();
                if var_name.starts_with(prefix) {
                    let part = rest.split('#').next().unwrap_or("").trim();
                    let count = count_quoted_items(part);
                    if part.contains(')') {
                        results.insert(var_name.to_string(), count);
                    } else {
                        in_array = Some((var_name.to_string(), count));
                    }
                }
            }
        }
        results
    }

    fn count_quoted_items(s: &str) -> usize {
        let mut count = 0;
        let mut in_single = false;
        let mut in_double = false;
        let mut in_word = false;

        for c in s.chars() {
            if in_single {
                if c == '\'' {
                    in_single = false;
                    count += 1;
                }
            } else if in_double {
                if c == '"' {
                    in_double = false;
                    count += 1;
                }
            } else if c == '\'' {
                in_single = true;
                in_word = false;
            } else if c == '"' {
                in_double = true;
                in_word = false;
            } else if c == '(' || c == ')' || c.is_whitespace() {
                if in_word {
                    count += 1;
                    in_word = false;
                }
            } else {
                in_word = true;
            }
        }
        if in_word {
            count += 1;
        }
        count
    }

    fn count_srcinfo_lines(content: &str, prefix: &str) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some((key, _)) = trimmed.split_once('=') {
                let key = key.trim();
                if key.starts_with(prefix) {
                    *counts.entry(key.to_string()).or_insert(0) += 1;
                }
            }
        }
        counts
    }

    /// A `)` inside a message must not end the call early and hide the
    /// argument after it — the shape a scanner that counts every paren misses.
    #[test]
    fn calls_ignore_parens_inside_literals() {
        let code =
            r##"println!("step 1) {}", path.display()); other(); println!(r#"a ) "b" "#, x);"##;
        let found = super::calls(code, "println!(");
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].ends_with("path.display())"), "{}", found[0]);
        assert!(found[1].ends_with("x)"), "{}", found[1]);
    }

    #[test]
    fn calls_handle_char_literals_lifetimes_and_escapes() {
        let code = r#"print!("{}", f(')', '"', "\"", &'a str)); tail()"#;
        let found = super::calls(code, "print!(");
        assert_eq!(found, [r#"print!("{}", f(')', '"', "\"", &'a str))"#]);
    }

    /// `"println!("` is a substring of `eprintln!(`; each call is still one
    /// hit, and an unbalanced tail neither panics nor loops.
    #[test]
    fn calls_visit_each_macro_once_and_survive_an_unclosed_call() {
        let code = "eprintln!(\"a\"); println!(\"b\"); println!(\"c\"";
        let found = super::calls(code, "println!(");
        assert_eq!(
            found,
            ["println!(\"a\")", "println!(\"b\")", "println!(\"c\""]
        );
    }
}
