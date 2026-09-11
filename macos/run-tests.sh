#!/usr/bin/env bash
# Run the pure-logic test harness for the menu bar app.
#
# The app file is compiled with -D SWIFT_TEST_HARNESS, which strips its @main
# entry point (app.run()); the test file supplies its own @main TestRunner
# instead, so the combined module has exactly one entry point. No Xcode project
# or XCTest bundle is needed. Internal helpers are reached directly (no `public`
# ceremony).
#
# Run:  ./macos/run-tests.sh
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "› Compiling + running tests…"
mkdir -p "$TMP/cache"
swiftc -module-cache-path "$TMP/cache" -O -parse-as-library -D SWIFT_TEST_HARNESS \
  "$DIR/ai-usagebar-menubar.swift" \
  "$DIR/ai-usagebar-tests.swift" \
  -o "$TMP/ai-usagebar-tests"

"$TMP/ai-usagebar-tests"
