// Contract tests for the applet's pure model. Run with
// `node plasma/model.test.mjs` (or `make desktop-test`). Model.js is loaded the same way the QML engine loads it:
// as a plain script with no module system around it.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const uiDir = new URL('./package/contents/ui/', import.meta.url);
const source = fs.readFileSync(new URL('Model.js', uiDir), 'utf8');
// Evaluated in this realm (so arrays and objects compare natively) but inside
// a function scope, the way the QML engine hands the file its own scope.
const exported = [
  'cleanText', 'safeText', 'finitePercent', 'normalizeSection', 'normalizeEntry',
  'parseReport', 'parseSettings', 'baseProvider', 'isUnconfigured', 'cycleIds',
  'stepId', 'entryById', 'formatDuration', 'resetRemainingMs', 'elapsedMs',
  'parseDetail', 'splitPair', 'pangoToHtml'
];
const model = vm.runInThisContext(
  `(function () {\n${source}\n; return { ${exported.join(', ')} }; })()`,
  { filename: 'Model.js' }
);

// --- text hardening ---------------------------------------------------------

assert.equal(model.cleanText('ab\u200fc\u0007', 100), 'abc');
assert.equal(model.cleanText('abcdef', 4), 'abc…');
assert.equal(model.cleanText(undefined, 100), '');
assert.equal(model.safeText('<img src=x>'), '‹img src=x›');
assert.equal(model.safeText('two\nlines'), 'two lines');

// --- report parsing ---------------------------------------------------------

const reportJson = JSON.stringify({
  primary: 'anthropic',
  entries: [
    {
      id: 'anthropic',
      name: 'anthropic',
      display_name: 'Claude',
      plan: 'Claude Pro',
      status: 'ready',
      error: null,
      stale: false,
      fetched_at: '2026-08-16T15:09:36Z',
      sections: [
        { type: 'spacer' },
        {
          type: 'metric',
          label: 'Session (5h)',
          percent: 30,
          value: '30%',
          severity: 'low',
          detail: 'Resets in 4h 09m · 16% elapsed · 14pts ahead',
          reset_at: '2026-08-16T19:20:00Z'
        },
        { type: 'block', label: 'Credits', body: ['balance: 0', 'unmapped line'] }
      ]
    },
    {
      id: 'zai',
      display_name: 'Z.AI',
      status: 'error',
      error: 'credentials error: Zai: no API key. Set `api_key` under [zai].',
      sections: []
    }
  ]
});

const report = model.parseReport(reportJson);
assert.equal(report.ok, true);
assert.equal(report.primary, 'anthropic');
assert.equal(report.entries.length, 2);

const claude = report.entries[0];
// Spacers are dropped by the model so the QML repeater never renders one.
assert.deepEqual(claude.sections.map(s => s.type), ['metric', 'block']);
assert.equal(claude.display_name, 'Claude');
assert.equal(claude.status, 'ready');
assert.equal(claude.sections[0].reset_at, '2026-08-16T19:20:00Z');
assert.deepEqual(claude.sections[1].body, ['balance: 0', 'unmapped line']);

// An entry carrying an error is an error entry even when status says otherwise.
assert.equal(report.entries[1].status, 'error');

// Severity is recomputed when the binary omits (or drifts on) the field.
const drifted = model.parseReport(JSON.stringify({
  entries: [{ id: 'x', sections: [{ type: 'metric', percent: 95, severity: 'wat' }] }]
}));
assert.equal(drifted.entries[0].sections[0].severity, 'critical');

assert.deepEqual(model.parseReport('not json'), { ok: false, reason: 'invalid-json', primary: '', entries: [] });
assert.equal(model.parseReport('{"nope":1}').reason, 'unsupported');
assert.equal(model.parseReport('{"entries":[{"no":"id"}]}').reason, 'empty');

// --- settings ---------------------------------------------------------------

assert.deepEqual(model.parseSettings('{"schema_version":1,"primary":"openai"}'), { ok: true, primary: 'openai' });
assert.deepEqual(model.parseSettings('{"schema_version":1,"primary":"../evil"}'), { ok: true, primary: '' });
assert.equal(model.parseSettings('{"schema_version":9}').ok, false);
assert.equal(model.parseSettings('boom').ok, false);

// --- provider selection -----------------------------------------------------

assert.equal(model.isUnconfigured(report.entries[1]), true);
assert.equal(model.isUnconfigured(claude), false);

const ids = model.cycleIds(report.entries, true);
assert.deepEqual(ids, ['anthropic']);
assert.deepEqual(model.cycleIds(report.entries, false), ['anthropic', 'zai']);

const three = ['a', 'b', 'c'];
assert.equal(model.stepId(three, 'a', true), 'b');
assert.equal(model.stepId(three, 'c', true), 'a');
assert.equal(model.stepId(three, 'a', false), 'c');
// An unknown (e.g. newly pinned) provider must not strand the cycle.
assert.equal(model.stepId(three, 'zzz', true), 'a');
assert.equal(model.stepId([], 'a', true), '');

assert.equal(model.entryById(report.entries, 'zai').id, 'zai');
assert.equal(model.entryById(report.entries, '').id, 'anthropic');
assert.equal(model.entryById([], 'anthropic'), null);
// Multi-account ids ("anthropic@work") fall back to the base provider.
assert.equal(model.entryById(report.entries, 'anthropic@work').id, 'anthropic');

// --- durations and timestamps ----------------------------------------------

assert.equal(model.formatDuration(45 * 1000), '1m');
assert.equal(model.formatDuration(9 * 60 * 1000), '9m');
assert.equal(model.formatDuration((4 * 60 + 9) * 60 * 1000), '4h 09m');
assert.equal(model.formatDuration((3 * 24 + 7) * 3600 * 1000), '3d 7h');
assert.equal(model.formatDuration(-1), '');
assert.equal(model.formatDuration('nope'), '');

const now = Date.parse('2026-08-16T15:00:00Z');
assert.equal(model.resetRemainingMs('2026-08-16T16:00:00Z', now), 3600000);
assert.equal(model.resetRemainingMs('2026-08-16T14:00:00Z', now), -3600000);
assert.equal(model.resetRemainingMs('', now), null);
assert.equal(model.resetRemainingMs('not a date', now), null);
assert.equal(model.elapsedMs('2026-08-16T14:59:00Z', now), 60000);
// A clock that ran backwards must not print a negative age.
assert.equal(model.elapsedMs('2026-08-16T15:10:00Z', now), 0);
assert.equal(model.elapsedMs('', now), null);

// --- detail line ------------------------------------------------------------

const detail = model.parseDetail('Resets in 4h 09m · 16% elapsed · 14pts ahead');
assert.equal(detail.reset, 'Resets in 4h 09m');
assert.equal(detail.elapsed, 16);
assert.equal(detail.pacePoints, 14);
assert.equal(detail.paceDirection, 'ahead');
assert.deepEqual(detail.extras, []);

assert.equal(model.parseDetail('52% elapsed · 27pts under').paceDirection, 'under');
assert.equal(model.parseDetail('12pts behind').paceDirection, 'under');
assert.equal(model.parseDetail('3pts over').paceDirection, 'ahead');
assert.equal(model.parseDetail('on pace').onPace, true);
// Anything the frontend does not recognize is passed through untouched.
assert.deepEqual(model.parseDetail('Resets in 1h · brand new field').extras, ['brand new field']);
assert.deepEqual(model.parseDetail('').extras, []);

assert.deepEqual(model.splitPair('balance: 0'), { key: 'balance', value: '0' });
assert.deepEqual(model.splitPair('no colon here'), { key: 'no colon here', value: '' });

// --- pango markup -----------------------------------------------------------

assert.equal(
  model.pangoToHtml("<span foreground='#98c379'>57%</span>"),
  '<span style="color:#98c379;">57%</span>'
);
assert.equal(
  model.pangoToHtml("<span font_weight='bold'>x</span>"),
  '<span style="font-weight:bold;">x</span>'
);
// Markup that is not a pango span is escaped, never forwarded to Qt rich text.
assert.equal(model.pangoToHtml('<img src=\'x\'>'), '&lt;img src=\'x\'&gt;');
assert.equal(model.pangoToHtml("<span foreground='url(x)'>y</span>"), '<span style="">y</span>');
assert.equal(model.pangoToHtml('a & b'), 'a &amp; b');
assert.equal(model.pangoToHtml(''), '');

// --- package contract -------------------------------------------------------

const metadata = JSON.parse(fs.readFileSync(new URL('./package/metadata.json', import.meta.url), 'utf8'));
assert.equal(metadata.KPackageStructure, 'Plasma/Applet');
assert.match(metadata.KPlugin.Id, /^[a-z0-9.]+$/);

// Translations only load when the catalog is named after the applet id.
const catalog = new URL(
  `./package/contents/locale/pt_BR/LC_MESSAGES/plasma_applet_${metadata.KPlugin.Id}.mo`,
  import.meta.url
);
assert.ok(fs.existsSync(catalog), 'missing compiled pt_BR catalog for ' + metadata.KPlugin.Id);

// Every configuration key the QML reads must exist in the KConfigXT schema.
const configXml = fs.readFileSync(new URL('./package/contents/config/main.xml', import.meta.url), 'utf8');
const declared = new Set([...configXml.matchAll(/entry name="([^"]+)"/g)].map(m => m[1]));
const qmlSources = fs.readdirSync(uiDir).filter(name => name.endsWith('.qml'))
  .map(name => fs.readFileSync(new URL(name, uiDir), 'utf8')).join('\n');
for (const match of qmlSources.matchAll(/plasmoid\.configuration\.([A-Za-z0-9_]+)/g)) {
  assert.ok(declared.has(match[1]), `configuration key not declared in main.xml: ${match[1]}`);
}

// The applet must keep talking to the binary through the documented commands.
const mainQml = fs.readFileSync(new URL('main.qml', uiDir), 'utf8');
assert.match(mainQml, /import "Model\.js" as Model/);
assert.match(mainQml, /usage --json/);
assert.match(mainQml, /settings show/);
// `--vendor` next to a subcommand is rejected by the binary's argument parser.
assert.doesNotMatch(mainQml, /--vendor[^\n]*\b(usage|settings)\b/);

console.log('ok - model contract');
