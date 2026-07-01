#!/usr/bin/env bash
# Install the menu bar app as a LaunchAgent so it starts at login.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$DIR/ai-usagebar-menubar"
LABEL="com.akitaonrails.ai-usagebar-menubar"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"

[ -x "$BIN" ] || { echo "Compile primeiro: $DIR/build.sh" >&2; exit 1; }

mkdir -p "$HOME/Library/LaunchAgents"

xml_escape() {
  local s=$1 out= c i
  for ((i = 0; i < ${#s}; i++)); do
    c=${s:i:1}
    case "$c" in
      '&') out+='&amp;' ;;
      '<') out+='&lt;' ;;
      '>') out+='&gt;' ;;
      '"') out+='&quot;' ;;
      *) out+="$c" ;;
    esac
  done
  printf '%s' "$out"
}

BIN_XML=$(xml_escape "$BIN")
cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>$BIN_XML</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
EOF

launchctl unload "$PLIST" 2>/dev/null || true
launchctl load "$PLIST"

echo "✓ $LABEL carregado (sobe no login)."
echo "  Parar:     launchctl unload \"$PLIST\""
echo "  Logs:      log stream --predicate 'process == \"ai-usagebar-menubar\"'"
