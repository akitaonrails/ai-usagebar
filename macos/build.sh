#!/usr/bin/env bash
# Build the ai-usagebar menu bar app (single-file, no Xcode project).
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

command -v swiftc >/dev/null || {
    echo "swiftc não encontrado. Instale as Command Line Tools:" >&2
    echo "  xcode-select --install" >&2
    exit 1
}

echo "› Building (swiftc -O)…"
swiftc -O "$DIR/ai-usagebar-menubar.swift" -o "$DIR/ai-usagebar-menubar"
echo "✓ Built: $DIR/ai-usagebar-menubar"
echo
echo "Rodar agora:        $DIR/ai-usagebar-menubar &"
echo "Subir no login:     $DIR/install-agent.sh"
