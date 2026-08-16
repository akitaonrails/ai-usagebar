#!/usr/bin/env bash
# Regenerates po/<domain>.pot from the QML sources and merges it into every
# existing catalog. Run it after adding or changing a user-visible string.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$here")"
domain="plasma_applet_$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["KPlugin"]["Id"])' "$root/package/metadata.json")"
pot="$here/$domain.pot"

cd "$root"

# xgettext has no QML parser; JavaScript is close enough for the i18n calls.
xgettext \
    --language=JavaScript \
    --from-code=UTF-8 \
    --package-name="AI Usage Bar" \
    --copyright-holder="ai-usagebar contributors" \
    --msgid-bugs-address="https://github.com/akitaonrails/ai-usagebar/issues" \
    --keyword=i18n:1 \
    --keyword=i18nc:1c,2 \
    --keyword=i18np:1,2 \
    --keyword=i18ncp:1c,2,3 \
    --output="$pot" \
    $(find package/contents -name '*.qml' | sort)

for po in "$here"/*.po; do
    [ -e "$po" ] || continue
    msgmerge --quiet --update --backup=none "$po" "$pot"
done

echo "wrote $pot"
