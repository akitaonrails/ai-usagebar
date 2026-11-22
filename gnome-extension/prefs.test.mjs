import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';

const source = readFileSync(new URL('./prefs.js', import.meta.url), 'utf8');

// The first page holds Display/Menu/Colours/Data/Position: it needs its own
// tab label and icon, not a blank one next to "Vendors".
const firstPage = source.match(/new Adw\.PreferencesPage\(\{[\s\S]*?\}\)/);
assert.ok(firstPage, 'the first page must be constructed with properties');
assert.match(firstPage[0], /title:/, 'the first page must carry a tab title');
assert.match(firstPage[0], /icon_name:/, 'the first page must carry a tab icon');

// Markup defaults differ between libadwaita versions. When disabling it, pass
// literal text; escaping a plain subtitle would display the literal &amp;.
assert.doesNotMatch(source, /subtitle:\s*GLib\.markup_escape_text/,
    'no subtitle may be wrapped in GLib.markup_escape_text');
const poolControls = source.match(/const pools = new Adw\.ComboRow\(\{[\s\S]*?\}\);/);
assert.match(poolControls?.[0] ?? '', /use_markup:\s*false/,
    'the literal ampersand in the panel-pools subtitle must not be parsed as markup');
assert.doesNotMatch(poolControls?.[0] ?? '', /subtitle:/,
    'assign the subtitle after the markup binding is initialized');

// Translatable strings are English; these Portuguese leftovers slipped in.
for (const pt of [
    'o custo extra ($) como terceira barra',
    'Pools no painel',
    'Antigravity detectado',
])
    assert.ok(!source.includes(pt), `Portuguese leftover in prefs.js: ${pt}`);

console.log('GNOME prefs tests passed');
