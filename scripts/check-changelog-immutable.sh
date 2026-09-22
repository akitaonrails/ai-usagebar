#!/usr/bin/env bash
#
# Every released CHANGELOG section must still match the tag that shipped it,
# and the version files must never move backwards.
#
# Why this is a script and not a #[test]: it needs `git show <tag>`, and tests
# must not shell out to git. The AUR `check()` runs `cargo test` from an
# extracted tarball with no history, and nix/package.nix filters CHANGELOG.md
# out of the source set entirely — the in-tree guards already have to return
# Option and skip for that reason.
#
# The hazard it catches: a branch that predates the last tag carries its
# entries under [Unreleased], which is exactly where the newest released
# section now sits, so git merges the two together *cleanly*. It has happened
# five times in this repo, in three different shapes — silently inserted into a
# published section, conflicted into one, and deleting one outright — and every
# time the merge looked successful.
set -uo pipefail
cd "$(dirname "$0")/.."

fail=0
tags=$(git tag --list 'v*' --sort=-v:refname | head -"${CHANGELOG_TAGS_TO_CHECK:-8}")
[ -z "$tags" ] && { echo "no v* tags found — nothing to compare"; exit 0; }

# Tags cut but abandoned before their release ever shipped: v1.20.0 was caught
# by verify-version with a stale Scoop manifest and never published, and the
# release that replaced it (1.20.1) folded its section away in the same
# stroke — no linear changelog can satisfy both that tag and v1.20.1, because
# each section's trailing next-heading is part of the comparison. The tag
# survives (repository rules forbid deleting it), so it is skipped here.
abandoned_tags="v1.20.0"

for tag in $tags; do
  case " $abandoned_tags " in
    *" $tag "*)
      echo "skip: $tag was never published — no section to keep intact"
      continue
      ;;
  esac
  v=${tag#v}
  a=$(git show "$tag:CHANGELOG.md" 2>/dev/null | sed -n "/^## \[$v\]/,/^## \[/p")
  b=$(sed -n "/^## \[$v\]/,/^## \[/p" CHANGELOG.md)
  if [ -z "$a" ]; then
    echo "warn: $tag has no [$v] section of its own — skipping"
    continue
  fi
  if [ -z "$b" ]; then
    # A release that was retired on purpose (v1.20.0 never shipped; its
    # entries moved under 1.20.1) has no `[X.Y.Z]:` compare link either. A
    # merge that drops a heading (the v1.13.0 case) leaves the link behind.
    if ! grep -q "^\[$v\]: " CHANGELOG.md; then
      echo "warn: $tag is retired — no [$v] section or link in CHANGELOG.md — skipping"
      continue
    fi
    echo "error: the [$v] section is MISSING from CHANGELOG.md but exists in $tag"
    fail=1
    continue
  fi
  if [ "$a" != "$b" ]; then
    echo "error: the published [$v] section no longer matches $tag:"
    diff <(printf '%s\n' "$a") <(printf '%s\n' "$b") | sed 's/^/    /'
    fail=1
  fi
done

newest=$(git tag --list 'v*' --sort=-v:refname | head -1)
if [ -n "$newest" ]; then
  want=${newest#v}
  check_version() {
    got=$(sed -n "0,/$2/s//\1/p" "$1")
    [ -z "$got" ] && { echo "error: could not read a version from $1"; fail=1; return; }
    # A release PR bumps past the tag and must pass; going backwards must not.
    if [ "$(printf '%s\n%s\n' "$want" "$got" | sort -V | head -1)" != "$want" ]; then
      echo "error: $1 says $got, older than the released $want"
      fail=1
    fi
  }
  check_version Cargo.toml                  '^version = "\(.*\)"'
  check_version manifest.json               '.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*'
  check_version packaging/aur/PKGBUILD      '^pkgver=\(.*\)'
  check_version packaging/aur/PKGBUILD-bin  '^pkgver=\(.*\)'
fi

[ "$fail" = 0 ] && echo "released changelog sections and version files are intact"
exit $fail
