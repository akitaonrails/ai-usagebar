// Table tests for the plasmoid's pure layer, in the same bare style as
// gnome-extension/marker-logic.test.mjs: node:assert/strict, no framework, no
// dependency. Run with `node kde-plasmoid/plasmoid-logic.test.mjs`.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {FORMAT} from './package/contents/code/marker-logic.mjs';
import {
    buildArgv, buildCommand, DEFAULT_BINARY, DEFAULT_VENDOR, nextVendor,
    barSegments, buildTuiCommand, cellParts, compactCells, detailRows, KNOWN_VENDORS, paletteFromTheme,
    parseUsageReport, vendorChoices, parseFields, parseWidgetJson, shellQuote, toRows,
    VENDOR_AUTH, buildVendorProbeCommand, parseVendorProbe, vendorAuthRows,
    oauthCommand, vendorActionCommand, vendorLabel,
} from './package/contents/code/plasmoid-logic.mjs';

const at = rel => fileURLToPath(new URL(rel, import.meta.url));

// ---------------------------------------------------------------------------
// The copy must never drift from the canonical file.
//
// This is deliberately a Node check rather than `cmp` in the Makefile: it also
// runs on the Windows CI job, which has neither make nor cmp. None of the drift
// this guards against produces a stack trace — a plainTextFromPango fix landing
// in only one copy silently leaves one frontend open to markup injection, and a
// stale hasUsageWindows deny-list makes the plasmoid draw confident 0% bars for
// a balance-only vendor.
// ---------------------------------------------------------------------------
const canonical = readFileSync(at('../gnome-extension/marker-logic.js'));
const copy = readFileSync(at('./package/contents/code/marker-logic.mjs'));
assert.ok(
    canonical.equals(copy),
    'kde-plasmoid/package/contents/code/marker-logic.mjs has drifted from the ' +
    'canonical gnome-extension/marker-logic.js.\nFix with:\n' +
    '  cp gnome-extension/marker-logic.js kde-plasmoid/package/contents/code/marker-logic.mjs',
);

// ---------------------------------------------------------------------------
// V4 portability. Both of these shipped as real bugs during Fase 0, found only
// by running the code under a live Plasma applet — Node accepts both happily.
//
//   catch {   → QML's V4 engine rejects the ES2019 optional catch binding with
//               a bare "Syntax error" and the whole module fails to load.
//   \p{...}   → V4 evaluates Unicode property escapes to FALSE instead of
//               throwing. That made poolTag() return '' for every pool, so the
//               Plasma panel silently rendered Antigravity's two quota pools
//               with no distinguishing tag. Silent wrong answers, no error.
//
// Asserting on the source text keeps this in the Node-only gate, so CI catches
// it on every platform without needing Qt installed.
// ---------------------------------------------------------------------------
// Whole-line comments are dropped first, so the comments explaining these very
// rules don't trip them. Good enough for a source heuristic; the authoritative
// check is running the module under a real applet (contents/ui/main.qml).
const codeOnly = src => src.split('\n').filter(l => !/^\s*(\/\/|\*|\/\*)/.test(l)).join('\n');

for (const [label, raw] of [
    ['marker-logic.mjs', copy.toString('utf8')],
    ['plasmoid-logic.mjs', readFileSync(at('./package/contents/code/plasmoid-logic.mjs'), 'utf8')],
]) {
    const src = codeOnly(raw);
    assert.ok(!/\\p\{/.test(src),
        `${label} uses a Unicode property escape (\\p{...}). QML's V4 engine ` +
        `evaluates those to false silently — test for "not space or punctuation" instead.`);
    assert.ok(!/\}\s*catch\s*\{/.test(src),
        `${label} uses the optional catch binding (catch {). QML's V4 engine ` +
        `rejects it — write catch (e) { instead.`);
}

// ---------------------------------------------------------------------------
// The config schema. Both of these fail SILENTLY at runtime, which is why they
// are asserted here rather than left to review:
//
//   * A double hyphen inside an XML comment is illegal. It made the whole
//     main.xml unparseable, so every Plasmoid.configuration.* read came back
//     undefined — with no error anywhere except one "Unable to assign
//     [undefined] to QString" line. Option names are therefore written without
//     their leading dashes in that file.
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

const configUi = readFileSync(at('./package/contents/ui/configGeneral.qml'), 'utf8')
    + readFileSync(at('./package/contents/ui/configVendors.qml'), 'utf8');
const entryNames = [...configXml.matchAll(/<entry\s+name="([^"]+)"/g)].map(m => m[1]);
assert.ok(entryNames.length >= 9, `expected the full schema, found ${entryNames.length} entries`);
for (const name of entryNames) {
    assert.ok(new RegExp(`cfg_${name}\\b`).test(configUi),
        `config/main.xml declares "${name}" but no config page has cfg_${name}. ` +
        `Plasma would silently never persist it.`);
}

// ---------------------------------------------------------------------------
// nextVendor — the scroll ring (per-instance, never the global active_vendor)
// ---------------------------------------------------------------------------
const ring = ['anthropic', 'openai', 'zai'];
assert.equal(nextVendor(ring, 'anthropic', 1), 'openai');
assert.equal(nextVendor(ring, 'openai', 1), 'zai');
assert.equal(nextVendor(ring, 'zai', 1), 'anthropic', 'wraps forward');
assert.equal(nextVendor(ring, 'anthropic', -1), 'zai', 'wraps backward');
assert.equal(nextVendor(ring, 'zai', -1), 'openai');
// A vendor dropped from config has no meaningful neighbour.
assert.equal(nextVendor(ring, 'deepseek', 1), 'anthropic');
assert.equal(nextVendor(['anthropic'], 'anthropic', 1), 'anthropic', 'single-element ring');
assert.equal(nextVendor(['anthropic'], 'anthropic', -1), 'anthropic');
assert.equal(nextVendor([], 'anthropic', 1), 'anthropic', 'empty ring is a no-op');
assert.equal(nextVendor(null, 'anthropic', 1), 'anthropic');
// A fast flick can accumulate more than one step.
assert.equal(nextVendor(ring, 'anthropic', 4), 'openai');
assert.equal(nextVendor(ring, 'anthropic', -4), 'zai');

// A KConfigXT StringList reaches QML as an array-LIKE object: right length,
// right contents, but Array.isArray() === false. Guarding on isArray made every
// scroll a silent no-op in the real panel while every test here still passed —
// so the ring must be exercised with a non-Array too, not just a literal.
const arrayLike = {0: 'anthropic', 1: 'openai', 2: 'zai', length: 3};
assert.equal(Array.isArray(arrayLike), false, 'the fixture must not be a real Array');
assert.equal(nextVendor(arrayLike, 'anthropic', 1), 'openai',
    'an array-like ring (what KConfig actually hands QML) must still cycle');
assert.equal(nextVendor(arrayLike, 'zai', 1), 'anthropic', 'and must still wrap');
assert.equal(nextVendor(arrayLike, 'anthropic', -1), 'zai');
// A bare string is not a ring — Array.from would split it into characters.
assert.equal(nextVendor('anthropic', 'anthropic', 1), 'anthropic');
assert.equal(nextVendor({length: 0}, 'anthropic', 1), 'anthropic');
assert.equal(nextVendor({}, 'anthropic', 1), 'anthropic');

// ---------------------------------------------------------------------------
// shellQuote / buildCommand — the KShell AbortOnMeta trap
// ---------------------------------------------------------------------------
assert.equal(shellQuote('plain'), `'plain'`);
assert.equal(shellQuote(`it's`), `'it'\\''s'`, 'embedded quote is escaped, not dropped');
assert.equal(shellQuote(''), `''`);
assert.equal(shellQuote(null), `''`);

// FORMAT is exactly the payload that trips KShell::splitArgs(AbortOnMeta) —
// it must come back as one fully single-quoted argument.
const cmd = buildCommand('', 'openai', FORMAT);
assert.ok(cmd.includes(`'${FORMAT}'`), 'FORMAT must be single-quoted verbatim');
assert.ok(!/[;{}]/.test(cmd.replace(/'[^']*'/g, '')),
    'no shell metacharacter may appear outside a quoted span');

// ---------------------------------------------------------------------------
// buildArgv — the regression guard against reintroducing active_vendor coupling
// ---------------------------------------------------------------------------
assert.deepEqual(
    buildArgv('/usr/bin/ai-usagebar', 'zai', 'F'),
    ['/usr/bin/ai-usagebar', '--vendor', 'zai', '--format', 'F'],
);
assert.deepEqual(buildArgv('', '', 'F'), [DEFAULT_BINARY, '--vendor', DEFAULT_VENDOR, '--format', 'F']);
assert.deepEqual(buildArgv(null, null, null), [DEFAULT_BINARY, '--vendor', DEFAULT_VENDOR, '--format', '']);
for (const args of [buildArgv('', '', ''), buildArgv(' ', ' ', ' '), buildArgv('b', 'v', 'f'),
    buildArgv('b', 'v', 'f', 60)]) {
    assert.ok(args.includes('--vendor'),
        '--vendor must always be explicit; without it the binary falls back to ' +
        'the shared active_vendor file and two plasmoid instances collide');
    assert.notEqual(args[args.indexOf('--vendor') + 1], '', '--vendor needs a value');
}

// The timeout(1) wrapper. The data engine gives QML no way to kill a hung
// child, so this is the only thing that actually bounds it — the QML watchdog
// merely stops listening. Omitting the argument must keep the bare call, so a
// caller that does not want the wrapper is unaffected.
assert.deepEqual(buildArgv('b', 'v', 'f', 60),
    ['timeout', '-k', '5', '60', 'b', '--vendor', 'v', '--format', 'f']);
assert.deepEqual(buildArgv('b', 'v', 'f'), ['b', '--vendor', 'v', '--format', 'f']);
for (const bad of [0, -1, null, undefined, NaN, 'soon', Infinity])
    assert.deepEqual(buildArgv('b', 'v', 'f', bad), ['b', '--vendor', 'v', '--format', 'f'],
        `a non-positive or non-finite timeout (${String(bad)}) must not wrap the call`);
assert.deepEqual(buildArgv('b', 'v', 'f', 45.6).slice(0, 4), ['timeout', '-k', '5', '46'],
    'seconds must reach timeout(1) as an integer');
// Quoting still holds with the wrapper in front.
const timedCmd = buildCommand('', 'openai', FORMAT, 60);
assert.ok(timedCmd.startsWith(`'timeout' '-k' '5' '60' `), 'wrapper must lead the command');
assert.ok(timedCmd.includes(`'${FORMAT}'`), 'FORMAT stays single-quoted verbatim');
assert.ok(!/[;{}]/.test(timedCmd.replace(/'[^']*'/g, '')),
    'no shell metacharacter may appear outside a quoted span');

// ---------------------------------------------------------------------------
// buildTuiCommand — here the shell metacharacters are wanted, not quoted away
// ---------------------------------------------------------------------------
assert.equal(buildTuiCommand('konsole -e'), `konsole -e 'ai-usagebar-tui'`);
assert.equal(buildTuiCommand('  '), buildTuiCommand(''), 'blank is the same as unset');
const chain = buildTuiCommand('');
for (const term of ['konsole', 'x-terminal-emulator', 'xdg-terminal-exec'])
    assert.ok(chain.includes(term), `fallback chain is missing ${term}`);
assert.ok(chain.includes('exit 127'), 'no terminal found must fail loudly, not silently');
// The binary name is still quoted even inside the chain.
assert.ok(chain.includes(`'ai-usagebar-tui'`));

// ---------------------------------------------------------------------------
// parseWidgetJson — the binary always exits 0 and always prints one JSON object
// ---------------------------------------------------------------------------
assert.deepEqual(
    parseWidgetJson('{"text":"<span foreground=\'#e5c07b\'>62%</span>","tooltip":"t","class":"high"}'),
    {ok: true, text: '62%', tooltip: 't', cls: 'high'},
);
assert.equal(parseWidgetJson('').ok, false);
assert.equal(parseWidgetJson('   ').ok, false);
assert.equal(parseWidgetJson('not json').ok, false);
assert.equal(parseWidgetJson('not json').error, 'invalid JSON');
assert.equal(parseWidgetJson('[]').ok, false, 'an array is not a widget payload');
assert.equal(parseWidgetJson('null').ok, false);
// The ⚠ fallback is valid JSON and must become an error *state*, not a crash.
const warn = parseWidgetJson('{"text":"⚠","tooltip":"boom","class":"critical"}');
assert.equal(warn.ok, true);
assert.equal(warn.text, '⚠');
assert.equal(warn.cls, 'critical');

// ---------------------------------------------------------------------------
// parseFields — short payloads are the binary's own message, not usage data
// ---------------------------------------------------------------------------
assert.deepEqual(parseFields('⚠'), {kind: 'message', text: '⚠'});
assert.deepEqual(parseFields('Loading…'), {kind: 'message', text: 'Loading…'});
assert.equal(parseFields('a;;b;;c').kind, 'message', 'too few fields is never data');

// A full payload, in FIELD order.
const full = [
    'Claude Max 20x', '62', '1h 30m', '41', '3d 2h',   // plan, session, weekly
    '12', '5d', '30', '$3.00', '$10.00',               // sonnet, extra money
    '', '', '',                                        // scoped (absent)
    '55', '38', '',                                    // elapsed
    'cld',                                             // vendor_short
    '', '', '',                                        // extra model/reset/elapsed
    '', '',                                            // session/weekly model
    '__aiub_end__',
].join(';;');
const parsed = parseFields(full);
assert.equal(parsed.kind, 'data');
const d = parsed.data;
assert.equal(d.plan, 'Claude Max 20x');
assert.equal(d.vendorShort, 'cld');
assert.equal(d.hasUsageWindows, true);
assert.equal(d.grouped, false, 'no session_model means one pool, flat rows');
assert.deepEqual(d.session, {pct: 62, reset: '1h 30m', model: '', elapsed: 55});
assert.deepEqual(d.weekly, {pct: 41, reset: '3d 2h', model: '', elapsed: 38});
assert.equal(d.sonnet.label, 'Sonnet only');
assert.equal(d.sonnet.pct, 12);
assert.equal(d.extra.spent, '$3.00');
assert.equal(d.extra.limit, '$10.00');

// Grouped vendor (Antigravity): named pools switch the UI to grouped rows.
const grouped = toRows(full.split(';;').map((v, i) => (i === 20 ? 'Gemini' : i === 21 ? 'Gemini' : v)));
assert.equal(grouped.grouped, true);
assert.equal(grouped.session.model, 'Gemini');

// A scoped model is the presence signal for the per-model weekly cap.
const withScoped = full.split(';;');
withScoped[10] = 'Opus';       // scopedModel
withScoped[11] = '77';         // scopedPct
withScoped[12] = '2d';         // scopedReset
withScoped[15] = '60';         // scopedElapsed
const scoped = toRows(withScoped);
assert.deepEqual(scoped.sonnet, {pct: 77, reset: '2d', model: 'Opus', label: 'Opus', elapsed: 60});

// Malformed scoped data is "unavailable", never a silent fallback to Sonnet.
const badScoped = full.split(';;');
badScoped[10] = 'Opus';
badScoped[11] = 'nonsense';
const bad = toRows(badScoped);
assert.equal(bad.sonnet.pct, null);
assert.equal(bad.sonnet.label, 'Opus');
assert.equal(bad.sonnet.reset, '—');

// An older binary echoes unknown placeholders back; field() must discard them
// rather than render "{weekly_model}" as a pool name.
const older = full.split(';;');
older[20] = '{session_model}';
older[21] = '{weekly_model}';
const legacy = toRows(older);
assert.equal(legacy.grouped, false);
assert.equal(legacy.session.model, '');

// Balance-only vendors expose no rolling windows — never fabricate 0% bars.
const dsk = full.split(';;');
dsk[16] = 'dsk';
assert.equal(toRows(dsk).hasUsageWindows, false);

// ---------------------------------------------------------------------------
// compactCells — the panel projection
// ---------------------------------------------------------------------------
assert.deepEqual(compactCells(null), [], 'no data means no cells, never a 0% bar');

const flat = compactCells(d);
// showExtra defaults to OFF, matching show-extra in GNOME and macOS.
assert.deepEqual(flat.map(c => c.tag), ['5h', '7d']);
assert.deepEqual(flat[0], {tag: '5h', pct: 62, elapsed: 55});
const withExtra = compactCells(d, {showExtra: true});
assert.deepEqual(withExtra.map(c => c.tag), ['5h', '7d', 'ex']);
// The money budget keeps its percentage colour but shows the amount.
assert.equal(withExtra[2].text, '$3.00');
assert.equal(withExtra[2].pct, 30);
// elapsed is -1, not null/undefined: QML needs a plain int.
assert.equal(withExtra[2].elapsed, -1);
assert.deepEqual(compactCells(d, {showWeekly: false, showExtra: true}).map(c => c.tag),
    ['5h', 'ex']);

// Balance-only vendors must not produce confident 0% window bars.
assert.deepEqual(compactCells(toRows(dsk)).filter(c => c.tag === '5h' || c.tag === '7d'), []);

// A grouped vendor's SECOND pool lives in the scoped + extra slots.
const twoPool = full.split(';;');
twoPool[20] = 'Gemini';        // sessionModel  -> primary pool name
twoPool[10] = 'Claude';        // scopedModel   -> secondary pool name
twoPool[11] = '44';            // scopedPct     -> secondary session
twoPool[12] = '6h';            // scopedReset
twoPool[16] = 'agy';
const grouped2 = compactCells(toRows(twoPool));
assert.ok(grouped2.length > 0, 'a grouped vendor must still render cells');
// Tags come from the pool names and must differ, or two pools look identical.
const tagLetters = [...new Set(grouped2.map(c => c.tag.split(' ')[0]))];
assert.equal(tagLetters.length, 2, `expected two distinct pool tags, got ${JSON.stringify(tagLetters)}`);
assert.ok(tagLetters.every(t => t.length > 0),
    'empty pool tags are the \\p{L} regression — pools become indistinguishable');
assert.deepEqual(tagLetters, ['G', 'C']);

// ---------------------------------------------------------------------------
// detailRows — the popup / tooltip projection
// ---------------------------------------------------------------------------
assert.deepEqual(detailRows(null), []);
const rows = detailRows(d);
// Labels match the GNOME dropdown verbatim so the two panels read the same.
assert.deepEqual(rows.map(r => r.label),
    ['Session', 'Weekly', 'Sonnet only', 'Extra usage']);
assert.ok(rows.every(r => r.kind === 'row'), 'a flat vendor gets no headings');
assert.equal(rows[0].pct, 62);
assert.equal(rows[0].reset, '1h 30m');
assert.equal(rows[0].elapsed, 55);
// The money budget shows spent / limit but keeps the pct for colouring.
assert.equal(rows[3].valueText, '$3.00 / $10.00');
assert.equal(rows[3].pct, 30);

// A window with no percentage is omitted, never drawn as a confident 0%.
assert.deepEqual(detailRows(toRows(dsk)).map(r => r.label), ['Sonnet only', 'Extra usage'],
    'balance-only vendors must not get 5h/7d rows');

// A grouped vendor gets Session/Weekly headings: with two independent pools,
// four flat rows give no clue which window belongs to which pool.
const groupedRows = detailRows(toRows(twoPool));
assert.deepEqual(groupedRows.filter(r => r.kind === 'heading').map(r => r.label),
    ['Session', 'Weekly']);
assert.equal(groupedRows[0].kind, 'heading');
assert.equal(groupedRows[0].label, 'Session');
// The second pool's 5h window sits under Session, not at the bottom.
assert.equal(groupedRows[1].label, 'Gemini');
assert.equal(groupedRows[2].label, 'Claude');

// Either pool can be absent or partial. The maintainer had to fix exactly this
// class of bug for the GNOME panel ("absent/partial secondary pools blanking the
// panel"), so a heading must never survive its own rows.
const mkFields = o => {
    const f = new Array(23).fill('');
    f[22] = '__aiub_end__';
    for (const k of Object.keys(o)) f[k] = o[k];
    return f;
};
// Grouped, but the SECOND pool has no windows at all.
const onlyPrimary = detailRows(toRows(mkFields(
    {0: 'Google AI Pro', 1: '12', 2: '4h', 3: '30', 4: '6d', 16: 'agy', 20: 'Gemini'})));
assert.deepEqual(onlyPrimary.map(r => r.kind + ':' + r.label),
    ['heading:Session', 'row:Gemini', 'heading:Weekly', 'row:Weekly']);
// Grouped, but only the second pool's 5h window exists: the Weekly group is
// empty, so its heading must not be emitted.
const onlySecondary = detailRows(toRows(mkFields(
    {0: 'Google AI Pro', 10: 'Claude', 11: '44', 12: '4h', 16: 'agy', 20: 'Gemini'})));
assert.deepEqual(onlySecondary.map(r => r.kind + ':' + r.label),
    ['heading:Session', 'row:Claude'],
    'a heading with no rows under it is the popup equivalent of a blank panel');
for (const rows of [onlyPrimary, onlySecondary]) {
    rows.forEach((r, i) => {
        if (r.kind !== 'heading') return;
        assert.ok(rows[i + 1] && rows[i + 1].kind === 'row',
            `heading "${r.label}" is not followed by a row`);
    });
}
// Malformed scoped data is unavailable, so it drops out rather than showing 0%.
assert.ok(!detailRows(bad).some(r => r.label === 'Opus'));
// A named scoped window uses the model name as its label.
assert.ok(detailRows(scoped).some(r => r.label === 'Opus' && r.pct === 77));

// ---------------------------------------------------------------------------
// vendorChoices — what the config page offers to put in the scroll ring
// ---------------------------------------------------------------------------
assert.equal(parseUsageReport('').ok, false);
assert.equal(parseUsageReport('garbage').ok, false);
assert.equal(parseUsageReport('{"nope":1}').ok, false);

// Shape taken verbatim from a real `ai-usagebar usage --json` run.
const report = parseUsageReport(JSON.stringify({entries: [
    {id: 'anthropic', name: 'anthropic', plan: 'Claude Max 20x', error: null},
    {id: 'openai', name: 'openai', plan: null, error: 'io error at ~/.codex/auth.json'},
    {id: 'zai', name: 'zai', plan: null, error: 'credentials error: Zai: no API key.\nsecond line'},
]}));
assert.equal(report.ok, true);

const choices = vendorChoices(report.entries);
const byId = Object.fromEntries(choices.map(c => [c.id, c]));
assert.equal(byId.anthropic.status, 'ok');
assert.equal(byId.anthropic.detail, 'Claude Max 20x');
assert.equal(byId.openai.status, 'error');
assert.equal(byId.zai.detail, 'credentials error: Zai: no API key.', 'detail is one line only');
// A vendor absent from config.toml must still be offerable — Antigravity is not
// enabled by default, and it is exactly the one this user wanted.
assert.equal(byId.antigravity.status, 'unknown');
assert.equal(byId.antigravity.configured, false);
for (const slug of KNOWN_VENDORS)
    assert.ok(slug in byId, `known vendor ${slug} missing from the picker`);

// Named accounts collapse to the bare slug, because --vendor takes a slug.
const accounts = vendorChoices(parseUsageReport(JSON.stringify({entries: [
    {id: 'anthropic@work', plan: 'Max', error: null},
    {id: 'anthropic@home', plan: 'Pro', error: null},
]})).entries);
assert.equal(accounts.filter(c => c.id === 'anthropic').length, 1, 'accounts must dedupe');
assert.ok(!accounts.some(c => c.id.includes('@')), '--vendor never takes an account suffix');

// No report (binary missing) still offers the known set rather than nothing.
assert.deepEqual(vendorChoices([]).map(c => c.id), KNOWN_VENDORS);
assert.deepEqual(vendorChoices(null).map(c => c.id), KNOWN_VENDORS);

// ---------------------------------------------------------------------------
// Vendor login page — a port of gnome-extension/prefs.js. Every subtitle and
// button label below is asserted against the GNOME wording verbatim: a port
// that says something different is not a port.
// ---------------------------------------------------------------------------
assert.deepEqual(VENDOR_AUTH.map(v => v.id),
    ['anthropic', 'openai', 'antigravity', 'zai', 'openrouter', 'deepseek'],
    'table order and membership must match prefs.js');
assert.equal(vendorLabel('zai'), 'Z.AI (GLM)');
// vendorLabel reads VENDOR_AUTH rather than a second literal table, so the
// General picker and the Vendors page cannot drift apart on a rename. Pin that:
// every ring vendor must resolve to a real name, never echo its slug back.
for (const slug of KNOWN_VENDORS) {
    const label = vendorLabel(slug);
    assert.notEqual(label, slug, `vendorLabel(${slug}) fell through to the raw slug`);
    assert.equal(label, VENDOR_AUTH.find(v => v.id === slug).name,
        `vendorLabel(${slug}) must be the same string the Vendors page shows`);
}
// An unknown slug still degrades to itself rather than throwing.
assert.equal(vendorLabel('nope'), 'nope');
assert.equal(vendorLabel(undefined), '');

// ---------------------------------------------------------------------------
// barSegments — the two-segment bar, composed as barMarkup() composes it
// ---------------------------------------------------------------------------
// No marker: one plain fill, nothing after it.
for (const noMark of [-1, undefined, null, NaN, 101, -0.5]) {
    const s = barSegments(50, noMark, 200);
    assert.equal(s.hasMarker, false, `elapsed=${String(noMark)} must not draw a marker`);
    assert.equal(s.preWidth, 100);
    assert.equal(s.postWidth, 0);
}
// Behind the clock: everything sits before the marker, no overshoot segment.
const behind = barSegments(30, 60, 200);
assert.equal(behind.hasMarker, true);
assert.equal(behind.preWidth, 60);   // fill 60px, marker at 120px
assert.equal(behind.markerX, 120);
assert.equal(behind.postWidth, 0, 'nothing may be painted past the marker when under pace');
// Ahead of the clock: the split is what a single rectangle could not express.
const ahead = barSegments(62, 40, 200);
assert.equal(ahead.preWidth, 80, 'up to the marker keeps the absolute-usage colour');
assert.equal(ahead.postX, 80);
assert.equal(ahead.postWidth, 44, 'only the overshoot carries the pace colour');
assert.equal(ahead.preWidth + ahead.postWidth, 124, 'the two segments must tile the fill');
// The segments always tile the fill exactly, for every pct/elapsed pair.
for (let p = 0; p <= 100; p += 7)
    for (let e = 0; e <= 100; e += 11) {
        const s = barSegments(p, e, 160);
        assert.ok(Math.abs((s.preWidth + s.postWidth) - (160 * p / 100)) < 1e-9,
            `segments must tile the fill at pct=${p} elapsed=${e}`);
        assert.ok(s.preWidth >= 0 && s.postWidth >= 0, 'no negative segment');
        assert.ok(s.postX + s.postWidth <= 160 + 1e-9, 'must not overflow the track');
    }
// Degenerate widths must not produce NaN geometry.
for (const w of [0, -5, undefined, null, NaN]) {
    const s = barSegments(50, 40, w);
    for (const k of ['preWidth', 'markerX', 'postX', 'postWidth'])
        assert.equal(s[k], 0, `${k} must be 0 for width=${String(w)}`);
}
// Out-of-range pct is clamped, never extrapolated past the track.
assert.equal(barSegments(140, -1, 200).preWidth, 200);
assert.equal(barSegments(-40, -1, 200).preWidth, 0);

// The probe must never let a key cross the boundary — it emits booleans only.
const probeCmd = buildVendorProbeCommand();
for (const v of VENDOR_AUTH)
    assert.ok(probeCmd.includes(v.id), `probe is missing ${v.id}`);
assert.ok(probeCmd.startsWith('bash -lc '),
    'login shell: npm-global / nvm bins are absent from plasmashell PATH');
assert.ok(/\$\{ZAI_API_KEY:-\}/.test(probeCmd), 'api-key vendors check their env var');
assert.ok(probeCmd.includes('.claude/.credentials.json'));
assert.ok(probeCmd.includes('.codex/auth.json'));
assert.ok(probeCmd.includes('antigravity-ide'), 'all three Antigravity state dirs');
// The awk program is single-quoted for the shell, so a literal ' inside it would
// close the block; the quote is passed via -v instead. Verified against a real
// config.toml: a set key reads 1, a commented-out one reads 0.
assert.ok(!/awk[^|]*'[^']*'[^']*'/.test(probeCmd.replace(/'\\''/g, '')) ||
          probeCmd.includes("printf"), 'quote must not be a literal in the awk program');
assert.ok(probeCmd.includes("\\047"), 'the quote char comes from printf, not a literal');

assert.deepEqual(parseVendorProbe('anthropic:1:1\nzai:0:0\n'), {
    anthropic: {configured: true, cliInstalled: true},
    zai: {configured: false, cliInstalled: false},
});
assert.deepEqual(parseVendorProbe(''), {});
assert.deepEqual(parseVendorProbe('garbage'), {});

const rowsFor = probe => Object.fromEntries(vendorAuthRows(probe).map(r => [r.id, r]));

// Nothing known yet.
assert.equal(vendorAuthRows({})[0].subtitle, 'verificando…');

// OAuth: logged in / installed-but-not-logged / not installed.
let R = rowsFor({anthropic: {configured: true, cliInstalled: true}});
assert.equal(R.anthropic.subtitle, '✓ Configurado');
assert.equal(R.anthropic.button, 'Re-logar');

R = rowsFor({openai: {configured: false, cliInstalled: true}});
assert.equal(R.openai.subtitle, '⚠ Não logado — `codex login`');
assert.equal(R.openai.button, 'Logar');

R = rowsFor({openai: {configured: false, cliInstalled: false}});
assert.equal(R.openai.subtitle, '⚠ codex não instalado (instala em ~/.local, sem sudo)');
assert.equal(R.openai.button, 'Instalar + logar');

// API-key vendors.
R = rowsFor({zai: {configured: false, cliInstalled: false}});
assert.equal(R.zai.subtitle, '⚠ Sem API key — ZAI_API_KEY');
assert.equal(R.zai.button, 'Configurar (TUI)');
assert.equal(rowsFor({zai: {configured: true, cliInstalled: false}}).zai.subtitle, '✓ Configurado');

// Antigravity: the local-server vendor, three distinct states.
R = rowsFor({antigravity: {configured: true, cliInstalled: true}});
assert.equal(R.antigravity.subtitle, '✓ Antigravity detectado — mantenha o app, IDE ou agy aberto');
assert.equal(R.antigravity.button, 'Abrir agy');
R = rowsFor({antigravity: {configured: false, cliInstalled: true}});
assert.equal(R.antigravity.subtitle, 'agy instalado — abra uma sessão para disponibilizar a quota');
R = rowsFor({antigravity: {configured: false, cliInstalled: false}});
assert.equal(R.antigravity.subtitle, 'abra ou instale o app, a IDE ou o agy; não há login separado');
assert.equal(R.antigravity.button, 'Sem login separado');
assert.equal(R.antigravity.enabled, false, 'nothing to open when agy is absent');

// oauthCommand is copied verbatim; the ~/.local prefix is what avoids sudo.
const oc = oauthCommand(VENDOR_AUTH[0]);
assert.ok(oc.includes('npm i -g --prefix "$HOME/.local" @anthropic-ai/claude-code'));
assert.ok(oc.includes('export PATH="$HOME/.local/bin:$PATH";'));
assert.ok(oc.includes('read -p "Instalar agora? [y/N] " a;'), 'install must stay consent-gated');

// Actions route to a terminal; konsole first on KDE, user override wins.
assert.ok(vendorActionCommand(VENDOR_AUTH[0], '', '').includes('konsole'));
assert.ok(vendorActionCommand(VENDOR_AUTH[0], 'alacritty -e', '').startsWith('alacritty -e '));
assert.ok(vendorActionCommand(VENDOR_AUTH[2], '', '').includes("'agy'"), 'local vendor opens its CLI');
assert.ok(vendorActionCommand(VENDOR_AUTH[3], '', '/usr/bin/ai-usagebar-tui')
    .includes("'/usr/bin/ai-usagebar-tui'"), 'api-key vendors open the TUI');

// ---------------------------------------------------------------------------
// cellParts — the show-percent / show-bars rule, tested rather than re-derived
// in a QML binding. Both off must still draw the value: an empty segment is
// never the intent (same fallback as seg() in GNOME and macOS).
// ---------------------------------------------------------------------------
assert.deepEqual(cellParts(true, true), {value: true, bar: true});
assert.deepEqual(cellParts(true, false), {value: true, bar: false});
assert.deepEqual(cellParts(false, true), {value: false, bar: true});
assert.deepEqual(cellParts(false, false), {value: true, bar: false},
    'both off still renders the value rather than an empty segment');

// ---------------------------------------------------------------------------
// paletteFromTheme — the whole "follow the Plasma theme" requirement
// ---------------------------------------------------------------------------
const theme = {
    textColor: '#tx', neutralTextColor: '#nt', negativeTextColor: '#ng',
    positiveTextColor: '#ps', disabledTextColor: '#dz',
};
assert.deepEqual(paletteFromTheme(theme), {
    low: '#tx', mid: '#tx', high: '#nt', critical: '#ng', empty: '#dz',
});
// Every key marker-logic's colorForPct/colorForDelta reads must be present.
for (const key of ['low', 'mid', 'high', 'critical', 'empty'])
    assert.ok(key in paletteFromTheme(theme), `palette is missing ${key}`);
assert.deepEqual(Object.keys(paletteFromTheme(undefined)).sort(),
    ['critical', 'empty', 'high', 'low', 'mid']);

console.log('plasmoid logic tests passed');
