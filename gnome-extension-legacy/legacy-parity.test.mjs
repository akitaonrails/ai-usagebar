// The legacy tree is a mechanical port of ../gnome-extension. This test loads
// both copies and checks that they still export and compute the same values.
// It also guards the two GNOME 42-specific constraints that are easy to
// regress: no ESM syntax, and no destroy_all_children() in the menu repaint.

import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {
    existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync,
    readlinkSync, rmSync, symlinkSync, writeFileSync,
} from 'node:fs';
import {tmpdir} from 'node:os';
import {dirname, join} from 'node:path';
import {fileURLToPath} from 'node:url';
import vm from 'node:vm';

import * as marker from '../gnome-extension/marker-logic.js';
import * as report from '../gnome-extension/report-model.js';
import * as terminal from '../gnome-extension/terminal.js';

const here = dirname(fileURLToPath(import.meta.url));

function loadLegacy(name) {
    const source = readFileSync(join(here, `${name}.js`), 'utf8');
    const context = vm.createContext({console});
    new vm.Script(source, {filename: `${name}.js`}).runInContext(context);
    assert.ok(context.exports, `${name}.js must publish its helpers through exports`);
    return context.exports;
}

const legacyMarker = loadLegacy('marker-logic');
const legacyReport = loadLegacy('report-model');
const legacyTerminal = loadLegacy('terminal');

for (const [name, esm, legacy] of [
    ['marker-logic', marker, legacyMarker],
    ['report-model', report, legacyReport],
    ['terminal', terminal, legacyTerminal],
]) {
    assert.deepEqual(Object.keys(legacy).sort(), Object.keys(esm).sort(),
        `${name}: export surface drifted from the ESM frontend`);
}

const COLORS = {
    low: '#98c379',
    mid: '#e5c07b',
    high: '#d19a66',
    critical: '#e06c75',
    empty: '#3e4451',
};

for (const pct of [null, 0, 7, 52, 99, 100]) {
    for (const width of [0, 8, 20]) {
        for (const elapsed of [null, 0, 25, 80, 100]) {
            if (width === 0 && Number.isFinite(elapsed))
                continue;
            assert.equal(
                legacyMarker.barMarkup(pct, width, COLORS, elapsed),
                marker.barMarkup(pct, width, COLORS, elapsed),
                `barMarkup(${pct}, ${width}, ${elapsed})`);
        }
    }
}

for (const value of [null, '', '0', '52', '101', 'abc', 52, -1])
    assert.equal(legacyMarker.integer(value), marker.integer(value), `integer(${value})`);

for (const value of ['', 'plan', null, 5])
    assert.equal(legacyMarker.field(value), marker.field(value), `field(${value})`);

for (const [reset, elapsed] of [[null, 0], ['2026-11-20T18:00:00Z', 0],
    ['2026-11-20T18:00:00Z', 45], ['2026-11-20T18:00:00Z', 100]]) {
    assert.equal(legacyMarker.markerElapsed(reset, elapsed),
        marker.markerElapsed(reset, elapsed), `markerElapsed(${reset}, ${elapsed})`);
}

for (const pct of [0, 49, 50, 74, 75, 89, 90, 100])
    assert.equal(legacyMarker.colorForPct(pct, COLORS), marker.colorForPct(pct, COLORS),
        `colorForPct(${pct})`);

for (const delta of [-30, -10, 0, 10, 30])
    assert.equal(legacyMarker.colorForDelta(delta, COLORS), marker.colorForDelta(delta, COLORS),
        `colorForDelta(${delta})`);

for (const delta of [-30, 0, 30])
    assert.equal(legacyMarker.paceGlyph(delta), marker.paceGlyph(delta), `paceGlyph(${delta})`);

for (const vendor of ['cld', 'gpt', 'zai', 'synthetic', ''])
    assert.equal(legacyMarker.hasUsageWindows(vendor), marker.hasUsageWindows(vendor),
        `hasUsageWindows(${vendor})`);

for (const model of ['', 'gpt-5', null])
    assert.equal(legacyMarker.isGrouped(model), marker.isGrouped(model), `isGrouped(${model})`);

for (const text of ['', 'a;;b;;c', 'only'])
    assert.equal(JSON.stringify(legacyMarker.splitFormatOutput(text)),
        JSON.stringify(marker.splitFormatOutput(text)), `splitFormatOutput(${text})`);

for (const text of ['plain', '<span foreground="#fff">5h 52%</span>'])
    assert.equal(legacyMarker.plainTextFromPango(text), marker.plainTextFromPango(text),
        `plainTextFromPango(${text})`);

for (const value of [null, '', '0', '52', '101', 'abc', 52, -1])
    assert.equal(legacyReport.finitePercent(value), report.finitePercent(value),
        `finitePercent(${value})`);

for (const ms of [0, 60000, 3599999, 3600000, 90061000])
    assert.equal(legacyReport.formatDuration(ms), report.formatDuration(ms), `formatDuration(${ms})`);

const NOW = Date.parse('2026-11-20T15:00:00Z');
for (const reset of [null, '2026-11-20T17:00:00Z', '2026-11-20T14:00:00Z'])
    assert.equal(legacyReport.formatReset(reset, NOW), report.formatReset(reset, NOW),
        `formatReset(${reset})`);

for (const [reset, window] of [['2026-11-20T17:00:00Z', 18000],
    ['2026-11-20T17:00:00Z', null], [null, 18000]]) {
    assert.equal(legacyReport.elapsedPercent(reset, window, NOW),
        report.elapsedPercent(reset, window, NOW), `elapsedPercent(${reset}, ${window})`);
}

for (const [detail, reset] of [['Resets in 2h 30m · 45% used', '2026-11-20T17:00:00Z'],
    ['plain detail', null], ['Regenerates 10:30 in 2h · note', '2026-11-20T17:00:00Z']]) {
    assert.equal(legacyReport.metricDetail(detail, reset), report.metricDetail(detail, reset),
        `metricDetail(${detail})`);
}

for (const [brand, id] of [['openai', 'openai'], ['', 'custom@acct'], ['Bad Slug', '']])
    assert.equal(legacyReport.brandSlug(brand, id), report.brandSlug(brand, id),
        `brandSlug(${brand}, ${id})`);

assert.equal(legacyReport.errorLine('could not run', 'ENOENT'), report.errorLine('could not run', 'ENOENT'));
assert.equal(JSON.stringify(legacyReport.commandFailure('no provider')),
    JSON.stringify(report.commandFailure('no provider')));

const FIXTURE = JSON.stringify({
    entries: [
        {
            id: 'openai',
            display_name: 'Codex',
            brand: 'openai',
            plan: 'ChatGPT Plus',
            sections: [
                {type: 'metric', label: 'Codex 5h', percent: 59,
                    reset_at: '2026-11-20T17:00:00Z', window_secs: 18000,
                    detail: 'Resets in 2h · 45% elapsed',
                    elapsed_percent: 45, projected_percent: 131, spark: '▁▂▃',
                    spark_points: [12, 18, 24]},
                {type: 'metric', label: 'Codex weekly', percent: 9,
                    reset_at: '2026-11-23T17:00:00Z', window_secs: 604800,
                    detail: 'Resets in 3d'},
                {type: 'block', label: 'Credits', body: ['Full reset (weekly + 5h)']},
            ],
        },
        {id: 'zai', name: 'zai', error: 'no API key', sections: []},
    ],
});
assert.equal(JSON.stringify(legacyReport.parseReport(FIXTURE, NOW)),
    JSON.stringify(report.parseReport(FIXTURE, NOW)));

for (const file of readdirSync(here).filter(f => f.endsWith('.js'))) {
    const source = readFileSync(join(here, file), 'utf8');
    assert.equal(/^\s*(?:import|export)\s/m.test(source), false,
        `${file} uses ESM syntax, which GNOME Shell 42 cannot load`);
    new vm.Script(source, {filename: file});
}

const extensionSource = readFileSync(join(here, 'extension.js'), 'utf8');
assert.equal(extensionSource.includes('destroy_all_children'), false,
    'destroy_all_children() aborts GNOME Shell 42 (st_bin_destroy)');

// Run the real menu selection path with shell widgets replaced by inert actors.
class Widget {
    clutter_text = {};
    children = [];
    add_child(child) {
        this.children.push(child);
    }
}
const extensionContext = vm.createContext({
    imports: {
        gi: {
            GObject: {Object: {find_property: () => null}, registerClass: value => value},
            St: {BoxLayout: Widget, Label: Widget},
            Pango: {WrapMode: {}, EllipsizeMode: {}},
        },
        misc: {extensionUtils: {getCurrentExtension: () => ({
            imports: {
                'marker-logic': {exports: legacyMarker},
                'report-model': {exports: legacyReport},
                terminal: {exports: legacyTerminal},
            },
        })}},
        ui: {panelMenu: {Button: class {}}},
    },
});
const Indicator = new vm.Script(extensionSource + '\nIndicator').runInContext(extensionContext);
const received = [];
const view = {
    _settings: {get_boolean: () => false},
    _configuredVendors: new Set(['openai']),
    _tab: 'openai@work',
    _header: entry => entry.id,
    _tabs: (entries, active) => received.push([Array.from(entries, e => e.id), active]),
    _entryBody: () => null,
    _colors: () => ({}),
    _updatedAgo: () => '',
    _swapPanel: () => {},
};
const expectedIds = ['openai@home', 'openai@work', 'custom:example@lab'];
const ready = legacyReport.parseReport(JSON.stringify({
    entries: expectedIds.map(id => ({id, name: id, sections: []})),
}), NOW);
Indicator.prototype._paintReport.call(view, ready);
assert.deepEqual(received, [[expectedIds, 'openai@work']]);
Indicator.prototype._paintReport.call(view, {ok: false, error: 'failed', entries: []});
assert.equal(received.length, 1, 'a failed report must not populate account tabs');

const metadata = JSON.parse(readFileSync(join(here, 'metadata.json'), 'utf8'));
assert.deepEqual(metadata['shell-version'], ['42', '43', '44']);
assert.equal(metadata.uuid, 'ai-usagebar@akitaonrails.github.io');
assert.equal(metadata['settings-schema'], 'org.gnome.shell.extensions.ai-usagebar');

const prefsSource = readFileSync(join(here, 'prefs.js'), 'utf8');
const poolControls = prefsSource.match(/const pools = new Adw\.ComboRow\(\{[\s\S]*?\}\);/);
assert.match(poolControls?.[0] ?? '', /HAS_USE_MARKUP \? \{use_markup:\s*false\}/);
assert.doesNotMatch(poolControls?.[0] ?? '', /subtitle:/);
assert.match(prefsSource,
    /pools\.subtitle\s*=\s*HAS_USE_MARKUP \? poolSubtitle : GLib\.markup_escape_text\(poolSubtitle, -1\)/);

if (process.platform === 'linux') {
    const tmp = mkdtempSync(join(tmpdir(), 'aiub-legacy-install-'));
    try {
        const installer = join(here, 'install.sh');
        const shim = join(tmp, 'bin');
        mkdirSync(shim);
        writeFileSync(join(shim, 'glib-compile-schemas'), '#!/bin/sh\nexit 0\n', {mode: 0o700});
        const run = home => spawnSync('bash', [installer], {
            env: {HOME: home, PATH: shim + ':/usr/bin:/bin'},
            encoding: 'utf8',
        });
        const destination = home => join(home, '.local/share/gnome-shell/extensions', metadata.uuid);
        const existing = join(tmp, 'existing');
        const old = destination(existing);
        mkdirSync(old, {recursive: true});
        writeFileSync(join(old, 'keep'), 'existing install');
        const conflict = run(existing);
        assert.equal(conflict.status, 1);
        assert.match(conflict.stderr, /Refusing to replace/);
        assert.equal(readFileSync(join(old, 'keep'), 'utf8'), 'existing install');
        const clean = join(tmp, 'clean');
        const target = destination(clean);
        for (let i = 0; i < 2; i++) {
            const result = run(clean);
            assert.equal(result.status, 0, result.stderr);
            assert.equal(readlinkSync(target), here);
        }
        const foreign = join(tmp, 'foreign');
        const other = destination(foreign);
        const missing = join(tmp, 'not-installed');
        mkdirSync(dirname(other), {recursive: true});
        symlinkSync(missing, other);
        const foreignLink = run(foreign);
        assert.equal(foreignLink.status, 1);
        assert.match(foreignLink.stderr, /Refusing to replace/);
        assert.equal(lstatSync(other).isSymbolicLink(), true);
        assert.equal(readlinkSync(other), missing);
        assert.equal(existsSync(missing), false);
    } finally {
        rmSync(tmp, {recursive: true, force: true});
    }
}

console.log('legacy parity: ok');
