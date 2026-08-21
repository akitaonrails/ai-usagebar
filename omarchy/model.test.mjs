import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const source = fs.readFileSync(new URL('./Model.js', import.meta.url), 'utf8');
const model = {};
vm.createContext(model);
vm.runInContext(source, model, {filename: 'Model.js'});

// Keep the marketplace/runtime shape in CI. The marketplace's structural
// validator only checks that the declared file exists; Quattro additionally
// needs the bar entry point to forward its nested panel lifecycle.
const manifest = JSON.parse(fs.readFileSync(new URL('../manifest.json', import.meta.url), 'utf8'));
assert.deepEqual(manifest.kinds, ['bar-widget']);
assert.equal(manifest.entryPoints.barWidget, 'omarchy/BarWidget.qml');
assert.equal(manifest.barWidget.defaults.showValue, true);
const showValueSchema = manifest.barWidget.schema.find(row => row.key === 'showValue');
assert.equal(showValueSchema.type, 'boolean');
assert.equal(showValueSchema.defaultValue, true);

const barWidgetSource = fs.readFileSync(new URL('./BarWidget.qml', import.meta.url), 'utf8');
assert.match(barWidgetSource, /^BarWidget\s*\{/m);
for (const method of ['open', 'close', 'toggle', 'closeForPopoutSwitch'])
  assert.match(barWidgetSource, new RegExp(`function\\s+${method}\\s*\\(`));
assert.match(barWidgetSource, /source:\s*Qt\.resolvedUrl\("Panel\.qml"\)/);
assert.match(barWidgetSource, /target\.anchorItem\s*=\s*button/);
assert.match(barWidgetSource, /target\.hostWidget\s*=\s*root/);
assert.match(barWidgetSource, /buttonCode\s*===\s*Qt\.RightButton\)\s*root\.launchDashboard\(\)/);
assert.doesNotMatch(barWidgetSource, /\bIpcHandler\s*\{/);

const panelSource = fs.readFileSync(new URL('./Panel.qml', import.meta.url), 'utf8');
assert.match(panelSource, /^Panel\s*\{/m);
assert.match(panelSource, /property\s+var\s+anchorItem:\s*null/);
assert.match(panelSource, /property\s+var\s+hostWidget:\s*null/);
assert.match(panelSource, /SettingsView\s*\{/);
assert.match(panelSource, /function\s+openSettings\s*\(/);
assert.match(panelSource, /setting\("lastSelectedEntryId",\s*""\)/);
assert.match(panelSource, /setting\("showValue",\s*true\)/);
assert.match(panelSource, /function\s+persistSelection\s*\(/);
assert.match(panelSource, /Model\.settingsWithOverrides\(root\.settings,\s*root\.moduleName,\s*values\)/);
assert.match(panelSource, /bar\.shell\.updateEntryInline\(root\.moduleName,\s*entry\)/);
assert.match(panelSource, /persistSelection\(selectedEntryId\)/);
assert.match(panelSource, /Model\.barLabel\(/);

const settingsViewSource = fs.readFileSync(new URL('./SettingsView.qml', import.meta.url), 'utf8');
assert.match(settingsViewSource, /command:\s*\["ai-usagebar",\s*"settings",\s*"show"\]/);
assert.match(settingsViewSource, /command:\s*\["ai-usagebar",\s*"settings",\s*"apply"\]/);
assert.match(settingsViewSource, /stdinEnabled:\s*true/);
assert.match(settingsViewSource, /write\(root\.pendingPayload\s*\+\s*"\\n"\)/);
assert.match(settingsViewSource, /signal\s+nousLoginRequested\(\)/);
assert.match(settingsViewSource, /signal\s+showValueRequested\(bool\s+enabled\)/);
assert.match(settingsViewSource, /label:\s*"Show usage value in the top bar"/);
assert.match(settingsViewSource, /Log in with Nous Research/);
assert.match(settingsViewSource, /Leave the terminal open until login completes/);
assert.match(settingsViewSource, /model:\s*root\.snapshot\.keys/);
assert.match(panelSource, /function\s+openNousLogin\s*\(/);
assert.match(panelSource, /ai-usagebar auth nous login/);
assert.match(panelSource, /onNousLoginRequested/);
assert.doesNotMatch(settingsViewSource, /command:\s*\[[^\]]*(?:api.?key|secret|pendingPayload)/i);

const raw = JSON.stringify({primary: 'openai', entries: [
  {
    id: 'anthropic@work',
    name: 'anthropic · work',
    display_name: 'Claude · work',
    plan: 'Claude Max 20x',
    status: 'ready',
    error: null,
    stale: true,
    fetched_at: '2026-08-14T12:00:00Z',
    sections: [
      {type: 'spacer'},
      {type: 'metric', label: 'Session (5h)', percent: 29, value: '29%',
       detail: 'Resets in 2h 0m · 60% elapsed · 31pts under', severity: 'low',
       reset_at: '2026-08-14T14:00:00Z'},
      {type: 'text', label: 'Balance', value: '$12.00'},
      {type: 'block', label: 'Credits', body: ['balance: 20', '≈ 10 messages']}
    ]
  },
  {
    id: 'openai', name: 'openai', display_name: 'Codex', plan: 'Plus', error: null,
    sections: [{type: 'metric', label: 'Codex weekly', percent: 95, value: '95%', detail: '', severity: 'critical'}]
  }
]});

const parsed = model.parseReport(raw);
assert.equal(parsed.ok, true);
assert.equal(parsed.primary, 'openai');
assert.equal(parsed.entries.length, 2);
assert.equal(parsed.entries[0].stale, true);
assert.equal(parsed.entries[0].sections[1].reset_at, '2026-08-14T14:00:00Z');
assert.equal(model.providerName(parsed.entries[0]), 'Claude · work');
assert.equal(model.providerName(parsed.entries[1]), 'Codex');
assert.deepEqual(Array.from(model.filteredEntries(parsed.entries, '')).map(entry => entry.id), ['anthropic@work', 'openai']);
assert.deepEqual(Array.from(model.filteredEntries(parsed.entries, 'anthropic')).map(entry => entry.id), ['anthropic@work']);
assert.deepEqual(Array.from(model.filteredEntries(parsed.entries, 'openai')).map(entry => entry.id), ['openai']);
assert.equal(model.selectedIndex(parsed.entries, 'openai'), 1);
assert.equal(model.selectedIndex(parsed.entries, 'missing'), 0);
assert.equal(model.preferredEntryId(parsed.entries, parsed.primary), 'openai');
assert.equal(model.preferredEntryId(parsed.entries, 'anthropic'), 'anthropic@work');
assert.equal(model.preferredEntryId(parsed.entries, 'missing'), 'anthropic@work');
assert.equal(model.preferredEntryId(parsed.entries, parsed.primary, 'anthropic@work'), 'anthropic@work');
assert.equal(model.preferredEntryId(parsed.entries, parsed.primary, '  ANTHROPIC@WORK  '), 'anthropic@work');
assert.equal(model.preferredEntryId(parsed.entries, parsed.primary, 'missing'), 'openai');

const openRouterAccounts = model.parseReport(JSON.stringify({entries: [{
  id: 'openrouter@work', name: 'openrouter · work', display_name: 'OpenRouter · work',
  error: null, sections: []
}, {
  id: 'openrouter@personal', name: 'openrouter · personal', display_name: 'OpenRouter · personal',
  error: null, sections: []
}]})).entries;
assert.deepEqual(Array.from(model.filteredEntries(openRouterAccounts, 'openrouter')).map(entry => entry.id),
  ['openrouter@work', 'openrouter@personal']);
assert.equal(model.providerName(openRouterAccounts[0]), 'OpenRouter · work');
assert.equal(model.preferredEntryId(openRouterAccounts, 'openrouter', 'openrouter@personal'),
  'openrouter@personal');
assert.equal(model.preferredEntryId(openRouterAccounts, 'openrouter', 'openrouter@missing'),
  'openrouter@work');

const priorWidgetSettings = {
  provider: '', refreshIntervalSec: 90, futureSetting: {keep: true}, id: 'stale-id'
};
const selectedWidgetSettings = model.settingsWithSelectedEntry(
  priorWidgetSettings, 'akitaonrails.ai-usagebar', 'openrouter@personal');
assert.deepEqual(JSON.parse(JSON.stringify(selectedWidgetSettings)), {
  id: 'akitaonrails.ai-usagebar',
  provider: '',
  refreshIntervalSec: 90,
  futureSetting: {keep: true},
  lastSelectedEntryId: 'openrouter@personal'
});
assert.equal(priorWidgetSettings.lastSelectedEntryId, undefined);
assert.equal(model.settingsWithSelectedEntry({}, 'akitaonrails.ai-usagebar', ''), null);
const hiddenValueSettings = model.settingsWithOverrides(
  selectedWidgetSettings, 'akitaonrails.ai-usagebar', {showValue: false});
assert.equal(hiddenValueSettings.showValue, false);
assert.equal(hiddenValueSettings.lastSelectedEntryId, 'openrouter@personal');
assert.equal(selectedWidgetSettings.showValue, undefined);
const protectedSettings = model.settingsWithOverrides({}, 'akitaonrails.ai-usagebar', {
  id: 'wrong-id', constructor: 'ignored', prototype: 'ignored', showValue: false
});
assert.equal(protectedSettings.id, 'akitaonrails.ai-usagebar');
assert.notEqual(protectedSettings.constructor, 'ignored');
assert.equal(protectedSettings.prototype, undefined);
assert.equal(model.booleanSetting(undefined, true), true);
assert.equal(model.booleanSetting(false, true), false);
assert.equal(model.booleanSetting('false', true), false);
assert.equal(model.booleanSetting('true', false), true);
assert.equal(model.booleanSetting('invalid', true), true);

assert.equal(model.barLabel(false, false, true, false, true, '29%'), '󰚩  29%');
assert.equal(model.barLabel(false, false, false, false, true, '29%'), '󰚩');
assert.equal(model.barLabel(true, false, true, false, true, '95%'), '󰚩  95%');
assert.equal(model.barLabel(true, false, false, false, true, '95%'), '󰚩');
assert.equal(model.barLabel(true, false, true, false, false, ''), '󰅙');
assert.equal(model.barLabel(false, true, true, false, true, '29%'), '󰚩');
assert.equal(model.barLabel(true, true, true, false, true, '95%'), '󰅙');
assert.equal(model.barLabel(false, false, true, true, false, ''), '󰚩  …');

assert.equal(model.headline(parsed.entries[0]).text, '29%');
assert.equal(model.headline(parsed.entries[1]).severity, 'critical');
assert.equal(model.isAlarming(parsed.entries[0]), true); // stale
assert.equal(model.isAlarming(parsed.entries[1]), true); // critical
assert.equal(model.formatReset('2026-08-14T14:00:00Z', Date.parse('2026-08-14T12:00:00Z')), 'Resets in 2h 0m');
assert.equal(model.formatUpdated('2026-08-14T12:00:00Z', Date.parse('2026-08-14T12:03:00Z')), 'Updated 3m ago');
assert.equal(model.metricDetail(parsed.entries[0].sections[1]), '60% elapsed · 31pts under');

const balance = model.parseReport(JSON.stringify({entries: [{
  id: 'deepseek', error: null,
  sections: [{type: 'text', label: 'Balance', value: '$8.42'}]
}]})).entries[0];
assert.equal(model.headline(balance).text, '$8.42');
const meteredBalance = model.parseReport(JSON.stringify({entries: [{
  id: 'openrouter', error: null,
  sections: [{type: 'metric', label: 'Credit balance', percent: 25, value: '$75.00', detail: ''}]
}]})).entries[0];
assert.equal(model.headline(meteredBalance).text, '$75.00');

assert.equal(model.parseReport('{').ok, false);
assert.equal(model.parseReport('{}').ok, false);
assert.equal(model.parseReport('{"entries":[{"name":"missing id"}]}').ok, false);
assert.equal(model.cleanText('bad\u0000value', 20), 'badvalue');
assert.equal(model.cleanText('tab\tcarriage\rC1\u0085value', 40), 'tab carriage C1value');
assert.equal(model.cleanText('😀😀', 3), '😀…');
assert.equal(model.autoTextSafe('<img src="https://example.test/pixel">'),
  '‹img src="https://example.test/pixel"›');
assert.equal(model.autoTextSafe('line\nspoof\u202eright-to-left'), 'line spoofright-to-left');
assert.equal(model.providerName({id: 'anthropic', display_name: 'Claude · <b>work</b>'}),
  'Claude · ‹b›work‹/b›');
assert.equal(model.providerName({id: 'openai', name: 'openai'}), 'openai');
assert.equal(model.errorMessage(''), 'The usage command failed without an error message.');

// A missing ai-usagebar binary must be reported as such, with the install
// command, instead of surfacing the helper's raw "not found" text or leaving
// the widget silently stuck on its loading state.
assert.match(model.launchErrorMessage(127, 'env: ai-usagebar: No such file or directory'),
  /ai-usagebar is not installed/);
assert.match(model.launchErrorMessage(127, ''), /omarchy pkg aur add ai-usagebar-bin/);
// Every other failure keeps the existing behaviour.
assert.equal(model.launchErrorMessage(1, 'boom'), 'boom');
assert.equal(model.launchErrorMessage(0, ''), 'The usage command failed without an error message.');

// The usage command must stay behind a helper that can emit exit 127 when the
// binary is absent, without opening a shell-injection boundary.
assert.match(panelSource,
  /command:\s*\["\/usr\/bin\/env",\s*"ai-usagebar",\s*"usage",\s*"--json"\]/);
assert.doesNotMatch(panelSource, /command:\s*\["(?:\/usr\/bin\/)?(?:ba)?sh"/);
assert.match(panelSource, /onExited:\s*function\(exitCode\)/);
assert.match(panelSource, /Model\.launchErrorMessage\(/);

const settingsRaw = JSON.stringify({
  schema_version: 1,
  primary: 'openai',
  primary_choices: [
    {id: 'anthropic', label: 'Claude'},
    {id: 'openai', label: 'Codex'}
  ],
  keys: [
    {id: 'kimi', label: 'Kimi', environment: 'KIMI_API_KEY', note: 'coding-plan usage',
     configured: true, inline_configured: true, environment_configured: false}
  ]
});
const settings = model.parseSettingsSnapshot(settingsRaw);
assert.equal(settings.ok, true);
assert.equal(settings.primary, 'openai');
assert.equal(settings.primary_choices[0].id, 'anthropic');
assert.equal(settings.primary_choices[0].value, 'anthropic');
assert.equal(settings.primary_choices[0].label, 'Claude');
assert.equal(settings.primary_choices[1].label, 'Codex');
assert.equal(settings.keys[0].inline_configured, true);
assert.equal(settings.keys[0].environment, 'KIMI_API_KEY');
const opencodeSettings = model.parseSettingsSnapshot(JSON.stringify({
  schema_version: 1,
  primary: 'opencode-go',
  primary_choices: [{id: 'opencode-go', label: 'OpenCode Go'}],
  keys: [{id: 'opencode-go', label: 'OpenCode Go', environment: 'OPENCODE_GO_API_KEY',
    note: 'usage quota', configured: false, inline_configured: false, environment_configured: false}]
}));
assert.equal(opencodeSettings.ok, true);
assert.equal(opencodeSettings.primary, 'opencode-go');
assert.equal(opencodeSettings.keys[0].id, 'opencode-go');
assert.equal(opencodeSettings.keys[0].environment, 'OPENCODE_GO_API_KEY');
assert.equal(model.parseSettingsSnapshot('{').ok, false);
assert.equal(model.parseSettingsSnapshot(JSON.stringify({schema_version: 2, primary_choices: [], keys: []})).ok, false);
const noEnabled = model.parseSettingsSnapshot(JSON.stringify({
  schema_version: 1, primary: 'anthropic', primary_choices: [], keys: []
}));
assert.equal(noEnabled.ok, true);
assert.equal(noEnabled.primary, '');

const patch = model.buildSettingsPatch('openai', [
  {id: 'kimi', action: 'set', value: 'secret-value'},
  {id: 'zai', action: 'clear'}
]);
assert.equal(patch.ok, true);
assert.deepEqual(JSON.parse(patch.payload), {
  schema_version: 1,
  primary: 'openai',
  keys: {
    kimi: {action: 'set', value: 'secret-value'},
    zai: {action: 'clear'}
  }
});
const keyOnlyPatch = model.buildSettingsPatch('', [{id: 'kimi', action: 'clear'}]);
assert.deepEqual(JSON.parse(keyOnlyPatch.payload), {
  schema_version: 1, keys: {kimi: {action: 'clear'}}
});
assert.equal(model.buildSettingsPatch('', []).ok, false);
assert.equal(model.buildSettingsPatch('openai', [{id: '__proto__', action: 'clear'}]).ok, false);
assert.equal(model.buildSettingsPatch('openai', [{id: 'kimi', action: 'set', value: ''}]).ok, false);
assert.equal(model.buildSettingsPatch('openai', [{id: 'kimi', action: 'bogus'}]).ok, false);
assert.equal(model.parseSettingsApplyResult('{"ok":true}'), true);
assert.equal(model.parseSettingsApplyResult('{"ok":false}'), false);

console.log('Omarchy model tests passed');
