// Table tests for the plasmoid's pure layer, in the same bare style as
// gnome-extension/marker-logic.test.mjs: node:assert/strict, no framework, no
// dependency. Run with `node kde-plasmoid/plasmoid-logic.test.mjs`.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {
    buildArgv, buildCommand, buildTuiCommand, cardFor, cardModel, cardState,
    DEFAULT_BINARY, DEFAULT_TIMEOUT_SECS,
    detailRows, entryFor, errorMessage, EXIT_KILLED, EXIT_TIMED_OUT, formatDuration,
    headline, isAlarming, MAX_TIMEOUT_SECS, MIN_TIMEOUT_SECS,
    metricDetail, nextVendor, paletteFromTheme, panelCells, parseReport,
    resetRemainingMs, safeText, severityColor, severityOf, SEVERITIES, shellQuote,
    shortLabel, shouldStartFetch, TIMEOUT_KILL_GRACE_SECS, timeoutSeconds,
    updatedAgeMs, vendorTabs,
} from './package/contents/code/plasmoid-logic.mjs';

const at = rel => fileURLToPath(new URL(rel, import.meta.url));

// ---------------------------------------------------------------------------
// V4 portability. Both of these shipped as real bugs during development and
// neither is caught by Node, which accepts them happily.
//
//   catch {   → QML's V4 engine rejects the ES2019 optional catch binding with
//               a bare "Syntax error" and the whole module fails to load.
//   \p{...}   → V4 evaluates Unicode property escapes to FALSE instead of
//               throwing. Silent wrong answers, no error anywhere.
//
// Asserting on the source text keeps this in the Node-only gate, so CI catches
// it on every platform without needing Qt installed.
// ---------------------------------------------------------------------------
// Whole-line comments are dropped first, so the comments explaining these very
// rules don't trip them. Good enough for a source heuristic; the authoritative
// check is running the module under a real applet (make mjs-probe).
const codeOnly = src => src.split('\n').filter(l => !/^\s*(\/\/|\*|\/\*)/.test(l)).join('\n');
const logicSrc = codeOnly(readFileSync(at('./package/contents/code/plasmoid-logic.mjs'), 'utf8'));

assert.ok(!/\\p\{/.test(logicSrc),
    'plasmoid-logic.mjs uses a Unicode property escape (\\p{...}). QML\'s V4 engine ' +
    'evaluates those to false silently — test for "not space or punctuation" instead.');
assert.ok(!/\}\s*catch\s*\{/.test(logicSrc),
    'plasmoid-logic.mjs uses the optional catch binding (catch {). QML\'s V4 engine ' +
    'rejects it — write catch (e) { instead.');

// ---------------------------------------------------------------------------
// The config schema. Both of these fail SILENTLY at runtime, which is why they
// are asserted here rather than left to review:
//
//   * A double hyphen inside an XML comment is illegal. It made the whole
//     main.xml unparseable, so every Plasmoid.configuration.* read came back
//     undefined — with no error anywhere except one "Unable to assign
//     [undefined] to QString" line.
//   * Plasma copies each config value onto a `cfg_<key>` property on the config
//     page root. A key with no matching alias simply never persists.
// ---------------------------------------------------------------------------
const configXml = readFileSync(at('./package/contents/config/main.xml'), 'utf8');
for (let i = configXml.indexOf('<!--'); i !== -1; i = configXml.indexOf('<!--', i + 4)) {
    const end = configXml.indexOf('-->', i + 4);
    assert.notEqual(end, -1, 'unterminated XML comment in config/main.xml');
    assert.ok(!configXml.slice(i + 4, end).includes('--'),
        `config/main.xml has a double hyphen inside the XML comment at offset ${i}. ` +
        `That is illegal and makes the ENTIRE schema unparseable, so every config ` +
        `default silently becomes undefined.`);
}

const configUi = readFileSync(at('./package/contents/ui/configGeneral.qml'), 'utf8');
const mainQml = readFileSync(at('./package/contents/ui/main.qml'), 'utf8');
const entryNames = [...configXml.matchAll(/<entry\s+name="([^"]+)"/g)].map(m => m[1]);
assert.ok(entryNames.length >= 9, `expected the full schema, found ${entryNames.length} entries`);
for (const name of entryNames) {
    assert.ok(new RegExp(`cfg_${name}\\b`).test(configUi),
        `config/main.xml declares "${name}" but no config page has cfg_${name}. ` +
        `Plasma would silently never persist it.`);
}
for (const removed of ['showSession', 'showWeekly', 'showExtra',
    'panelPools', 'panelAutoThreshold']) {
    assert.ok(!entryNames.includes(removed),
        `${removed} was a no-op setting and must not return to the schema`);
}

// All Label/Heading text sinks opt out of AutoText. Report strings may include
// provider-controlled text; AutoText can treat an <img> tag as rich text and
// fetch its source when the popup opens.
for (const rel of [
    './package/contents/ui/ColorSwatch.qml',
    './package/contents/ui/CompactRepresentation.qml',
    './package/contents/ui/FullRepresentation.qml',
    './package/contents/ui/UsageRow.qml',
    './package/contents/ui/UsageRows.qml',
    './package/contents/ui/VendorCards.qml',
    './package/contents/ui/configGeneral.qml',
]) {
    const src = readFileSync(at(rel), 'utf8');
    const labels = (src.match(/(?:Kirigami\.Heading|PlasmaComponents\.Label|QQC2\.Label)\s*\{/g) || []).length;
    const plain = (src.match(/textFormat:\s*Text\.PlainText/g) || []).length;
    assert.equal(plain, labels, `${rel} must make every Label/Heading plain text`);
}
assert.match(configUi, /prober\.connectSource\(Logic\.buildCommand\(/,
    'the settings probe must use the same bounded, shell-quoted command builder');
assert.match(configUi, /text:\s*page\.labelFor\(modelData\)/,
    'the current-vendor delegate must label its string model through labelFor');
assert.match(mainQml, /sourceName\s*!==\s*root\.pendingCommand/,
    'a completed command must be matched to the exact in-flight command');
assert.match(mainQml, /Logic\.panelCells\(root\.entry, \{max: 2\}\)/,
    'the compact view must not pretend a removed weekly toggle selects metrics');

// The Vendors page was removed when the report started carrying per-vendor
// status; config.qml must not still point at the deleted file, which Plasma
// reports only as an empty settings category.
const configModel = readFileSync(at('./package/contents/config/config.qml'), 'utf8');
for (const src of [...configModel.matchAll(/source:\s*"([^"]+)"/g)].map(m => m[1]))
    assert.doesNotThrow(() => readFileSync(at(`./package/contents/ui/${src}`)),
        `config.qml points at contents/ui/${src}, which does not exist`);

// ---------------------------------------------------------------------------
// A report shaped like the real one. Kept inline so the suite stays hermetic —
// it must never shell out to the binary or read a real cache.
// ---------------------------------------------------------------------------
const RAW = JSON.stringify({
    primary: 'openai',
    entries: [
        {
            id: 'anthropic', display_name: 'Claude', name: 'anthropic', plan: 'Max 20x',
            status: 'ready', stale: false, error: null,
            fetched_at: '2026-01-01T00:00:00Z',
            sections: [
                {type: 'spacer'},
                {type: 'metric', label: 'Session (5h)', value: '62%', percent: 62,
                    severity: 'mid', reset_at: '2026-01-01T02:00:00Z',
                    detail: 'Resets in 2h · 40% elapsed · 22pts over'},
                {type: 'spacer'},
                {type: 'metric', label: 'Weekly (7d)', value: '91%', percent: 91,
                    severity: 'critical', reset_at: '2026-01-04T00:00:00Z',
                    detail: 'Resets in 3d'},
            ],
        },
        {
            id: 'openai', display_name: 'Codex', name: 'openai', plan: 'Plus',
            status: 'ready', stale: true, error: null,
            fetched_at: '2026-01-01T00:00:00Z',
            sections: [{type: 'block', label: 'Credits', body: ['balance: $4.10']}],
        },
        {
            id: 'zai', display_name: 'Z.AI', name: 'zai', plan: '',
            status: 'error', stale: false, error: 'no API key', sections: [],
        },
    ],
});
const report = parseReport(RAW);
const NOW = Date.parse('2026-01-01T00:30:00Z');

assert.equal(report.ok, true);
assert.equal(report.entries.length, 3);
assert.equal(report.primary, 'openai');

// The binary always exits 0 and prints a report, so anything unparseable is a
// missing or broken binary — never a vendor-side failure.
for (const bad of ['', '   ', 'not json', '[]', '{}', 'null', '{"entries":null}'])
    assert.equal(parseReport(bad).ok, false, `must reject ${JSON.stringify(bad)}`);
assert.equal(parseReport('not json').raw, 'not json', 'the original output is kept for the error line');
// An entry with no id cannot be selected or tabbed to, so it is dropped rather
// than rendered as a nameless row.
assert.equal(parseReport(JSON.stringify({entries: [{plan: 'x'}]})).entries.length, 0);

// Number(null) === 0 in JS. A metric the report sends with percent null (a
// balance-style row, or a window the vendor does not report) must normalize to
// null so the views can show "not reported" — not to a fabricated 0% bar.
const nullPercent = parseReport(JSON.stringify({entries: [{id: 'v', sections: [
    {type: 'metric', label: 'Balance', value: '$4.10', percent: null},
]}]})).entries[0].sections[0];
assert.equal(nullPercent.percent, null);
assert.equal(nullPercent.severity, 'low');

// Provider-controlled strings cannot turn into QML rich text, preserve terminal
// controls, or use bidi overrides to disguise what the panel displays.
const hostile = parseReport(JSON.stringify({entries: [{
    id: 'hostile', display_name: '<img src="https://example.invalid/pixel">\u202eevil',
    plan: '<b>plan</b>', status: 'error', error: '\u001b[31mboom',
    sections: [{type: 'block', label: '<i>credits</i>', body: ['<img src="x">']}],
}]}));
const hostileText = JSON.stringify(hostile.entries[0]);
assert.ok(!/[<>\u001b\u202e]/.test(hostileText),
    'report normalization must remove rich-text delimiters and display controls');
assert.match(hostile.entries[0].label, /‹img src=/,
    'hostile markup is shown as inert text rather than silently discarded');

const oversized = parseReport(JSON.stringify({entries: Array.from({length: 80}, (_, i) => ({
    id: `v${i}`, sections: Array.from({length: 160}, () => ({
        type: 'block', body: Array.from({length: 160}, () => 'x'),
    })),
}))}));
assert.equal(oversized.entries.length, 64, 'entry rendering is bounded');
assert.equal(oversized.entries[0].sections.length, 128, 'section rendering is bounded');
assert.equal(oversized.entries[0].sections[0].body.length, 128, 'block rendering is bounded');

// ---------------------------------------------------------------------------
// severity
// ---------------------------------------------------------------------------
for (const s of SEVERITIES)
    assert.equal(severityOf(0, s), s, 'a declared severity always wins');
// Falls back to the documented 50/75/90 bands when the field is missing or
// unrecognised, so an older binary still colours sensibly.
assert.equal(severityOf(0, ''), 'low');
assert.equal(severityOf(49, undefined), 'low');
assert.equal(severityOf(50, 'nonsense'), 'mid');
assert.equal(severityOf(74, null), 'mid');
assert.equal(severityOf(75, ''), 'high');
assert.equal(severityOf(89, ''), 'high');
assert.equal(severityOf(90, ''), 'critical');
assert.equal(severityOf(null, ''), 'low', 'no percentage is not a crisis');

const colors = {low: 'L', mid: 'M', high: 'H', critical: 'C', empty: 'E'};
assert.equal(severityColor('low', colors), 'L');
assert.equal(severityColor('mid', colors), 'M');
assert.equal(severityColor('high', colors), 'H');
assert.equal(severityColor('critical', colors), 'C');
assert.equal(severityColor('nonsense', colors), 'L', 'an unknown severity must not be blank');
assert.equal(severityColor('low', undefined), undefined);

// ---------------------------------------------------------------------------
// entry selection
// ---------------------------------------------------------------------------
assert.equal(entryFor(report, 'zai').id, 'zai');
// A vendor dropped from config.toml must degrade to something visible, never a
// blank panel: the report's own primary, then the first entry.
assert.equal(entryFor(report, 'deepseek').id, 'openai', 'falls back to the report primary');
assert.equal(entryFor({entries: report.entries, primary: ''}, 'nope').id, 'anthropic');
assert.equal(entryFor(null, 'anthropic'), null);
assert.equal(entryFor({entries: []}, 'anthropic'), null);

// display_name is the canonical label; the raw id shows only if it is missing.
assert.deepEqual(vendorTabs(report, 'anthropic').map(t => t.label), ['Claude', 'Codex', 'Z.AI']);
assert.deepEqual(vendorTabs(report, 'anthropic').map(t => t.active), [true, false, false]);
// Every configured vendor is offered, including failing ones — hiding a broken
// vendor made "not configured" indistinguishable from "configured and broken".
assert.deepEqual(vendorTabs(report, 'anthropic').map(t => t.failing), [false, false, true]);
assert.equal(vendorTabs(report, 'gone').filter(t => t.active).length, 1,
    'an unknown id must still leave exactly one tab active');

// ---------------------------------------------------------------------------
// projection
// ---------------------------------------------------------------------------
const anthropic = entryFor(report, 'anthropic');
const openai = entryFor(report, 'openai');
const zai = entryFor(report, 'zai');

assert.equal(anthropic.label, 'Claude');
assert.equal(anthropic.plan, 'Max 20x');
assert.equal(openai.stale, true);
assert.equal(zai.status, 'error');

// The headline is the WORST window, not the first.
assert.equal(headline(anthropic).text, '91%');
assert.equal(headline(anthropic).severity, 'critical');
assert.equal(headline(anthropic).label, 'Weekly (7d)');
assert.equal(headline(zai).text, 'Error');
assert.equal(headline(null).text, '');

assert.equal(isAlarming(anthropic), true, 'a critical window is alarming');
assert.equal(isAlarming(openai), true, 'so is stale data');
assert.equal(isAlarming(zai), true, 'so is an errored vendor');
assert.equal(isAlarming(null), false);

// Spacers are dropped: Column spacing sets the rhythm, so keeping them would
// double it.
assert.equal(detailRows(anthropic).length, 2);
assert.deepEqual(detailRows(anthropic).map(r => r.type), ['metric', 'metric']);
assert.deepEqual(detailRows(openai).map(r => r.type), ['block']);
assert.deepEqual(detailRows(null), []);
// A block section keeps its free-form lines rather than being flattened away.
assert.deepEqual(detailRows(openai)[0].body, ['balance: $4.10']);

assert.deepEqual(panelCells(anthropic, {max: 2}).map(c => c.text), ['62%', '91%']);
assert.deepEqual(panelCells(anthropic, {max: 2}).map(c => c.label), ['5h', '7d']);
assert.deepEqual(panelCells(anthropic, {max: 1}).map(c => c.text), ['62%']);
assert.equal(panelCells(anthropic, {max: 2})[1].severity, 'critical');
// An errored vendor renders the same ⚠ the GNOME and macOS panels use, never a
// confident 0%.
assert.deepEqual(panelCells(zai).map(c => c.text), ['⚠']);
assert.deepEqual(panelCells(null), []);
// A vendor whose only section is a block has no percentage to plot.
assert.deepEqual(panelCells(openai), []);

// ---------------------------------------------------------------------------
// cards (viewMode "VendorCards")
// ---------------------------------------------------------------------------
const claudeCard = cardFor(anthropic);
const codexCard = cardFor(openai);
const zaiCard = cardFor(zai);

// One card per entry the aggregate report returned, in report order — the
// card view never restates a provider list of its own.
assert.deepEqual(cardModel(report).map(c => c.label), ['Claude', 'Codex', 'Z.AI']);
assert.equal(cardModel(null).length, 0);
assert.equal(cardFor(null), null);
assert.equal(cardState(null), 'ok');

// Windows are the metric sections, in report order; block sections ride along.
assert.deepEqual(claudeCard.windows.map(w => w.window), ['5h', '7d']);
assert.deepEqual(claudeCard.windows.map(w => w.value), ['62%', '91%']);
assert.deepEqual(claudeCard.windows.map(w => w.severity), ['mid', 'critical']);
assert.equal(claudeCard.windows[0].detail, '40% elapsed · 22pts over');
assert.equal(claudeCard.blocks.length, 0);
assert.equal(codexCard.blocks.length, 1);

// The accent is the WORST window — the same rule headline() applies.
assert.equal(claudeCard.accent, 'critical');

// Staleness is the report's own verdict (the Rust core owns it), and a stale
// card keeps the severity of its last good numbers instead of flipping red.
assert.equal(claudeCard.state, 'ok');
assert.equal(codexCard.state, 'stale');
assert.equal(codexCard.accent, 'low');
assert.deepEqual(codexCard.windows, []);

// An errored vendor outranks whatever its last good numbers said, replaces the
// gauges with the message, and never renders as a 0% bar.
assert.equal(zaiCard.state, 'error');
assert.equal(zaiCard.accent, 'critical');
assert.equal(zaiCard.error, 'no API key');
assert.deepEqual(zaiCard.windows, []);

// A window a vendor does not report carries percent null and its value text —
// the QML renders that as "not reported", never as a fabricated 0%.
const partial = cardFor(parseReport(JSON.stringify({entries: [{
    id: 'openrouter', display_name: 'OpenRouter', name: 'openrouter', plan: '',
    status: 'ready', stale: false, error: null,
    sections: [{type: 'metric', label: 'Session (5h)', value: '$1.20',
        percent: null, severity: 'low'}],
}]})).entries[0]);
assert.deepEqual(partial.windows.map(w => [w.percent, w.value]), [[null, '$1.20']]);
assert.equal(partial.accent, 'low');

assert.equal(shortLabel('Session (5h)'), '5h');
assert.equal(shortLabel('Weekly (7d)'), '7d');
// The parenthetical is the window descriptor, which is exactly the tag the
// panel wants — even when it is a word rather than a duration.
assert.equal(shortLabel('MCP tools (monthly)'), 'monthly');
// Nothing parenthesised, or something too long to be a window, falls back to
// the first word rather than to an empty tag.
assert.equal(shortLabel('Credits'), 'Credits');
assert.equal(shortLabel('Balance (since last invoice)'), 'Balance');
assert.equal(shortLabel(''), '');
assert.equal(shortLabel(undefined), '');

// detail still carries a "Resets in …" written for CLI readers; the popup
// renders a live countdown instead, so that fragment would duplicate and go
// stale.
assert.equal(metricDetail(detailRows(anthropic)[0]), '40% elapsed · 22pts over');
assert.equal(metricDetail(detailRows(anthropic)[1]), '', 'a reset-only detail collapses to nothing');
assert.equal(metricDetail(null), '');
assert.equal(metricDetail({detail: 'kept', resetAt: ''}), 'kept');

// ---------------------------------------------------------------------------
// time
// ---------------------------------------------------------------------------
assert.equal(formatDuration(0), 'now');
assert.equal(formatDuration(-1), 'now');
assert.equal(formatDuration(30 * 1000), '1m', 'under a minute still reads as 1m, never 0m');
assert.equal(formatDuration(62 * 60 * 1000), '1h 2m');
assert.equal(formatDuration(26 * 3600 * 1000), '1d 2h');
assert.equal(formatDuration('nonsense'), 'now');

assert.equal(resetRemainingMs('2026-01-01T02:00:00Z', NOW), 90 * 60 * 1000);
assert.ok(resetRemainingMs('2026-01-01T00:00:00Z', NOW) < 0, 'a past reset is negative, not clamped');
assert.equal(resetRemainingMs('', NOW), null);
assert.equal(resetRemainingMs('not a date', NOW), null);
assert.equal(resetRemainingMs(undefined, NOW), null);

assert.equal(updatedAgeMs('2026-01-01T00:00:00Z', NOW), 30 * 60 * 1000);
assert.equal(updatedAgeMs('2026-01-01T01:00:00Z', NOW), 0, 'a clock skew must not read as negative age');
assert.equal(updatedAgeMs('', NOW), null);
assert.equal(updatedAgeMs('not a date', NOW), null);

assert.equal(errorMessage(''), 'The usage command failed without an error message.');
assert.equal(errorMessage('  boom  '), 'boom');
assert.equal(safeText('<b>x</b>\u202e'), '‹b›x‹/b›');

// ---------------------------------------------------------------------------
// the scroll ring
// ---------------------------------------------------------------------------
const ring = ['anthropic', 'openai', 'zai'];
assert.equal(nextVendor(ring, 'anthropic', 1), 'openai');
assert.equal(nextVendor(ring, 'zai', 1), 'anthropic', 'wraps forward');
assert.equal(nextVendor(ring, 'anthropic', -1), 'zai', 'wraps backward');
assert.equal(nextVendor(ring, 'deepseek', 1), 'anthropic', 'a vendor dropped from config has no neighbour');
assert.equal(nextVendor([], 'anthropic', 1), 'anthropic', 'empty ring is a no-op');
assert.equal(nextVendor(null, 'anthropic', 1), 'anthropic');
assert.equal(nextVendor(ring, 'anthropic', 4), 'openai', 'a fast flick accumulates steps');

// A KConfigXT StringList reaches QML as an array-LIKE object: right length,
// right contents, but Array.isArray() === false. Guarding on isArray made every
// scroll a silent no-op in the real panel while every test here still passed.
const arrayLike = {0: 'anthropic', 1: 'openai', 2: 'zai', length: 3};
assert.equal(Array.isArray(arrayLike), false, 'the fixture must not be a real Array');
assert.equal(nextVendor(arrayLike, 'anthropic', 1), 'openai',
    'an array-like ring (what KConfig actually hands QML) must still cycle');
assert.equal(nextVendor(arrayLike, 'zai', 1), 'anthropic');

// ---------------------------------------------------------------------------
// the command
// ---------------------------------------------------------------------------
assert.equal(shellQuote('plain'), `'plain'`);
assert.equal(shellQuote(`it's`), `'it'\\''s'`, 'embedded quote is escaped, not dropped');
assert.equal(shellQuote(''), `''`);
assert.equal(shellQuote(null), `''`);

// One call covers every vendor, so --vendor never appears: the applet picks its
// entry client side and never reads the shared active_vendor file, which
// belongs to the Waybar module's --cycle-next.
for (const args of [buildArgv('', 0), buildArgv('b', 60)])
    assert.equal(args.indexOf('--vendor'), -1,
        'the plasmoid must never pass --vendor: that path reads the shared ' +
        'active_vendor file and two instances would collide');

// The timeout(1) wrapper. The data engine gives QML no way to kill a hung
// child, so this is the only thing that actually bounds it.
assert.deepEqual(buildArgv('b', 60),
    ['timeout', '-k', String(TIMEOUT_KILL_GRACE_SECS), '60', 'b', 'usage', '--json']);
assert.equal(timeoutSeconds(null), DEFAULT_TIMEOUT_SECS);
assert.equal(timeoutSeconds(undefined), DEFAULT_TIMEOUT_SECS);
assert.equal(timeoutSeconds('soon'), DEFAULT_TIMEOUT_SECS);
assert.equal(timeoutSeconds(0), MIN_TIMEOUT_SECS);
assert.equal(timeoutSeconds(45.6), MIN_TIMEOUT_SECS);
assert.equal(timeoutSeconds(600), 600);
assert.equal(timeoutSeconds(9999), MAX_TIMEOUT_SECS);
for (const bad of [0, -1, null, undefined, NaN, 'soon', Infinity]) {
    const args = buildArgv('b', bad);
    assert.equal(args[0], 'timeout', `timeout remains mandatory for ${String(bad)}`);
    assert.deepEqual(args.slice(-3), ['b', 'usage', '--json']);
}
assert.equal(EXIT_TIMED_OUT, 124);
assert.equal(EXIT_KILLED, 137);

// KShell::splitArgs(AbortOnMeta) refuses the whole string on an unquoted
// metacharacter, so every argument has to come back single-quoted.
const cmd = buildCommand('/opt/my apps/ai-usagebar', 60);
assert.ok(cmd.startsWith(`'timeout' '-k' '5' '60' `), 'the wrapper must lead the command');
assert.ok(cmd.includes(`'/opt/my apps/ai-usagebar'`), 'a path with spaces stays one argument');
assert.ok(!/[;{}]/.test(cmd.replace(/'[^']*'/g, '')),
    'no shell metacharacter may appear outside a quoted span');

// The TUI launcher is the opposite case: the metacharacters ARE the point,
// because there is no portable way to probe PATH from QML.
assert.ok(buildTuiCommand('').includes('||'), 'the fallback chain needs its ||');
assert.equal(buildTuiCommand('kitty -e'), `kitty -e 'ai-usagebar-tui'`);
assert.equal(buildTuiCommand('  '), buildTuiCommand(''), 'blank means "no custom terminal"');

assert.equal(shouldStartFetch('', 'cmd'), true);
assert.equal(shouldStartFetch('cmd', 'cmd'), false, 'never queue a second identical fetch');
assert.equal(shouldStartFetch('old', 'new'), false,
    'a config change must not start a second command while one is in flight');
assert.equal(shouldStartFetch('', ''), false, 'an empty command is never executable');

// ---------------------------------------------------------------------------
// theme
// ---------------------------------------------------------------------------
const palette = paletteFromTheme({
    textColor: 'T', neutralTextColor: 'N', negativeTextColor: 'G',
    positiveTextColor: 'P', disabledTextColor: 'D',
});
assert.equal(palette.low, 'P');
assert.equal(palette.critical, 'G');
assert.equal(palette.empty, 'D');
// Every role must resolve to something, or a theme missing one role paints an
// invisible bar.
for (const [key, value] of Object.entries(paletteFromTheme({textColor: 'T'})))
    assert.equal(value, 'T', `${key} must fall back to the plain text colour`);

console.log('plasmoid logic tests passed');
