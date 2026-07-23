#!/usr/bin/env bash
# Run the pure-logic test harness for the menu bar app.
#
# The app file is compiled with -D SWIFT_TEST_HARNESS, which strips its top-level
# entry point (app.run()); the test file then provides the only top-level code in
# the combined module, so Swift treats it as the main file. No Xcode project or
# XCTest bundle is needed. Internal helpers are reached directly (no `public`
# ceremony).
#
# Run:  ./macos/run-tests.sh
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "› Compiling + running tests…"
swiftc -O -parse-as-library -D SWIFT_TEST_HARNESS \
  "$DIR/ai-usagebar-menubar.swift" \
  "$DIR/ai-usagebar-tests.swift" \
  -o "$TMP/ai-usagebar-tests"

"$TMP/ai-usagebar-tests"
