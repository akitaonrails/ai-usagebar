#!/usr/bin/env bash
# Installs (or upgrades) the Plasma applet for the current user.
# The executable dependency is not installed here: get `ai-usagebar` first.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
id="com.akitaonrails.aiusagebar"
domain="plasma_applet_$id"

if ! command -v kpackagetool6 >/dev/null; then
    echo "kpackagetool6 not found — is this a Plasma 6 system?" >&2
    exit 1
fi

# Compile any catalog that is missing or older than its source.
if command -v msgfmt >/dev/null; then
    for po in "$here"/po/*.po; do
        [ -e "$po" ] || continue
        lang="$(basename "$po" .po)"
        mo="$here/package/contents/locale/$lang/LC_MESSAGES/$domain.mo"
        if [ ! -e "$mo" ] || [ "$po" -nt "$mo" ]; then
            mkdir -p "$(dirname "$mo")"
            msgfmt --check -o "$mo" "$po"
        fi
    done
else
    echo "msgfmt not found: keeping the committed translation catalogs." >&2
fi

if kpackagetool6 --type Plasma/Applet --list 2>/dev/null | grep -qx "$id"; then
    kpackagetool6 --type Plasma/Applet --upgrade "$here/package"
else
    kpackagetool6 --type Plasma/Applet --install "$here/package"
fi

echo
echo "Installed. Restart the shell to pick up the change:"
echo "  kquitapp6 plasmashell && (plasmashell &)"
echo "Then add the 'AI Usage Bar' widget to your panel."
