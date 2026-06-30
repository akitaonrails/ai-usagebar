#!/usr/bin/env bash
# Install the menu bar app as a LaunchAgent so it starts at login.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$DIR/ai-usagebar-menubar"
LABEL="com.akitaonrails.ai-usagebar-menubar"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"

[ -x "$BIN" ] || { echo "Compile primeiro: $DIR/build.sh" >&2; exit 1; }

mkdir -p "$HOME/Library/LaunchAgents"
sed "s#__BINARY__#$BIN#g" "$DIR/$LABEL.plist" > "$PLIST"

launchctl unload "$PLIST" 2>/dev/null || true
launchctl load "$PLIST"

echo "✓ $LABEL carregado (sobe no login)."
echo "  Parar:     launchctl unload \"$PLIST\""
echo "  Logs:      log stream --predicate 'process == \"ai-usagebar-menubar\"'"
