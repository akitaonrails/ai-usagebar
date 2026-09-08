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
// Opt-in, so an existing bar entry that has never seen this key keeps the
// label it has today.
assert.equal(manifest.barWidget.defaults.showProvider, false);
const showProviderSchema = manifest.barWidget.schema.find(row => row.key === 'showProvider');
assert.equal(showProviderSchema.type, 'boolean');
assert.equal(showProviderSchema.defaultValue, false);
assert.equal(manifest.barWidget.defaults.showAll, false);
const showAllSchema = manifest.barWidget.schema.find(row => row.key === 'showAll');
assert.equal(showAllSchema.type, 'boolean');
assert.equal(showAllSchema.defaultValue, false);

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
assert.match(panelSource, /setting\("showProvider",\s*false\)/);
assert.match(panelSource, /setting\("showAll",\s*false\)/);
assert.match(panelSource, /showProvider\s*\?\s*Model\.providerShort\(entry\)\s*:\s*""/);
assert.match(panelSource, /Model\.barStrip\(/);
assert.match(panelSource, /Model\.providerIcon\(entry\)/);
assert.match(panelSource, /BrandMark\s*\{/);
assert.match(panelSource, /Model\.brandIconFile\(root\.entry\)/);
assert.match(panelSource, /foreground:\s*root\.entryAlarming\s*\?\s*root\.urgent/);
assert.doesNotMatch(panelSource, /BrandMark[\s\S]*foreground:\s*root\.alarming\s*\?/m);
const brandMarkSource = fs.readFileSync(new URL('./BrandMark.qml', import.meta.url), 'utf8');
assert.match(brandMarkSource, /icons\/" \+ root\.brand/);
assert.ok(fs.existsSync(new URL('./icons/claude.svg', import.meta.url)));
assert.ok(fs.existsSync(new URL('./icons/openai.svg', import.meta.url)));
assert.ok(fs.existsSync(new URL('./icons/grok.svg', import.meta.url)));
assert.ok(fs.existsSync(new URL('./icons/copilot.svg', import.meta.url)));
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
assert.match(settingsViewSource, /password:\s*true/);
assert.match(settingsViewSource, /function\s+finishApply\s*\(\)\s*\{[\s\S]*?scrubSecrets\(\)[\s\S]*?if\s*\(applyExitCode/s);
assert.match(settingsViewSource, /signal\s+nousLoginRequested\(\)/);
assert.match(settingsViewSource, /signal\s+copilotLoginRequested\(\)/);
assert.match(settingsViewSource, /signal\s+showValueRequested\(bool\s+enabled\)/);
assert.match(settingsViewSource, /label:\s*"Show usage value in the top bar"/);
assert.match(settingsViewSource, /signal\s+showProviderRequested\(bool\s+enabled\)/);
assert.match(settingsViewSource, /label:\s*"Show provider name in the top bar"/);
assert.match(settingsViewSource, /signal\s+showAllRequested\(bool\s+enabled\)/);
assert.match(settingsViewSource, /label:\s*"Show all providers in the top bar"/);
assert.match(panelSource, /onShowAllRequested/);
assert.match(panelSource, /onShowProviderRequested/);
assert.match(settingsViewSource, /Log in with Nous Research/);
assert.match(settingsViewSource, /Log in with GitHub Copilot/);
assert.match(settingsViewSource, /choose GitHub Copilot as primary and save/);
assert.match(settingsViewSource, /model:\s*root\.snapshot\.keys/);
assert.match(settingsViewSource, /Paste\s*"\s*\+\s*\(keyCard\.modelData\.secret_label/);
assert.match(panelSource, /function\s+openNousLogin\s*\(/);
assert.match(panelSource, /ai-usagebar auth nous login/);
assert.match(panelSource, /onNousLoginRequested/);
assert.match(panelSource, /function\s+openCopilotLogin\s*\(/);
assert.match(panelSource, /gh auth login --web/);
assert.match(panelSource, /onCopilotLoginRequested/);
assert.doesNotMatch(settingsViewSource, /GitHub OAuth token|GITHUB_COPILOT_TOKEN/);
assert.doesNotMatch(settingsViewSource, /command:\s*\[[^\]]*(?:api.?key|secret|pendingPayload)/i);

const raw = JSON.stringify({primary: 'openai', entries: [
  {
    id: 'anthropic@work',
    name: 'anthropic · work',
    display_name: 'Claude · work',
    short_name: 'cld',
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
    id: 'openai', name: 'openai', display_name: 'Codex', short_name: 'gpt', plan: 'Plus', error: null,
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
const shownProviderSettings = model.settingsWithOverrides(
  hiddenValueSettings, 'akitaonrails.ai-usagebar', {showProvider: true});
assert.equal(shownProviderSettings.showProvider, true);
assert.equal(shownProviderSettings.showValue, false);
assert.equal(shownProviderSettings.lastSelectedEntryId, 'openrouter@personal');
assert.equal(hiddenValueSettings.showProvider, undefined);
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

// The provider tag is opt-in and arrives already resolved, so every call
// above — no seventh argument at all — has to keep its historical label.
assert.equal(model.barLabel(false, false, true, false, true, '29%', 'gpt'), '󰚩  gpt 29%');
// Tag on, value off: the icon-only label grows the tag and nothing else.
assert.equal(model.barLabel(false, false, false, false, true, '29%', 'gpt'), '󰚩  gpt');
assert.equal(model.barLabel(true, false, true, false, true, '95%', 'cld'), '󰚩  cld 95%');
// An entry with no headline still names its provider.
assert.equal(model.barLabel(false, false, true, false, true, '', 'agy'), '󰚩  agy');
// A vertical bar has no width for either field.
assert.equal(model.barLabel(false, true, true, false, true, '29%', 'gpt'), '󰚩');
// Before the first report there is no provider to name.
assert.equal(model.barLabel(false, false, true, true, false, '', 'gpt'), '󰚩  …');
assert.equal(model.barLabel(true, false, true, false, false, '', 'gpt'), '󰅙');
// A tag that sanitizes down to nothing degrades to the label without one.
assert.equal(model.barLabel(false, false, true, false, true, '29%', '   '), '󰚩  29%');
assert.equal(model.barLabel(false, false, true, false, true, '29%', undefined), '󰚩  29%');
assert.equal(model.barLabel(false, false, true, false, true, '100%', '', '󱢆'), '󱢆  100%');

assert.equal(model.brandIconFile({id: 'anthropic'}), 'claude.svg');
assert.equal(model.brandIconFile({id: 'anthropic@work'}), 'claude.svg');
assert.equal(model.brandIconFile({id: 'openai'}), 'openai.svg');
assert.equal(model.brandIconFile({id: 'supergrok'}), 'grok.svg');
assert.equal(model.brandIconFile({id: 'copilot'}), 'copilot.svg');
assert.equal(model.brandIconFile({id: 'kimi'}), 'kimi.svg');
assert.equal(model.brandIconFile({id: 'opencode-go'}), 'opencode.svg');
assert.equal(model.brandIconFile({id: 'commandcode'}), '');
assert.equal(model.brandIconFile({id: 'anthropic_api'}), 'anthropic.svg');
assert.equal(model.brandIconFile({id: 'grok'}), model.brandIconFile({id: 'supergrok'}));

const slugs = [
  'anthropic', 'anthropic_api', 'openai', 'copilot', 'zai', 'openrouter',
  'deepseek', 'kimi', 'kilo', 'novita', 'moonshot', 'grok', 'supergrok',
  'antigravity', 'cursor', 'minimax', 'kiro', 'nous', 'opencode-go', 'commandcode'
];
const byMark = {};
for (const slug of slugs) {
  const mark = model.brandIconFile({id: slug}) || slug;
  byMark[mark] = (byMark[mark] || []).concat(slug);
}
const sharedMarks = Object.entries(byMark).filter(([, vendors]) =>
  vendors.length > 1 && vendors.join() !== 'grok,supergrok');
assert.deepEqual(sharedMarks, []);
for (const slug of slugs) {
  const file = model.brandIconFile({id: slug});
  if (file) assert.ok(fs.existsSync(new URL('./icons/' + file, import.meta.url)), file);
}

const claudeChip = parsed.entries[0];
claudeChip.icon = '󰚩';
const openaiChip = parsed.entries[1];
openaiChip.icon = '󱢆';
const strip = model.barChips([claudeChip, openaiChip], openaiChip, true, true, false, false, false, false);
assert.equal(strip.length, 2);
assert.equal(strip[0].brand, 'claude.svg');
assert.equal(strip[1].brand, 'openai.svg');
assert.equal(strip[0].label, '29%');
assert.equal(strip[1].label, '95%');
const one = model.barChips([claudeChip, openaiChip], openaiChip, false, true, false, false, false, false);
assert.equal(one.length, 1);
assert.equal(one[0].brand, 'openai.svg');
assert.equal(model.barStrip([claudeChip, openaiChip], false, false, true, false, false), '󰚩  29%  󱢆  95%');

// The codes come from Rust's VendorId::short_name via the report; the vendor
// half of the machine id only stands in for a binary that predates the field.
assert.equal(model.providerShort(parsed.entries[0]), 'cld');
assert.equal(model.providerShort(parsed.entries[1]), 'gpt');
assert.equal(model.providerShort({id: 'anthropic@work'}), 'anthropic');
assert.equal(model.providerShort({id: 'anthropic_api'}), 'anthropic-api');
assert.equal(model.providerShort({id: 'zai', short_name: '   '}), 'zai');
assert.equal(model.providerShort(null), '');
// Provider-controlled text can never reach Text.AutoText as markup.
assert.equal(model.providerShort({id: 'x', short_name: '<b>x</b>'}), '‹b›x‹/b›');

assert.equal(model.headline(parsed.entries[0]).text, '29%');
assert.equal(model.headline(parsed.entries[1]).severity, 'critical');
assert.equal(model.isAlarming(parsed.entries[0]), true); // stale
assert.equal(model.isAlarming(parsed.entries[1]), true); // critical
// Reset-row fixtures are built from *local* calendar components, not UTC
// strings, so every expectation below is a literal that holds in any
// timezone the panel might run in. Deriving the expected clock from the same
// getHours()/getMinutes() expression the implementation uses would pass no
// matter what that expression did.
const localReset = (y, mo, d, h, mi) => new Date(y, mo - 1, d, h, mi).toISOString();
const at = (y, mo, d, h, mi) => Date.parse(new Date(y, mo - 1, d, h, mi).toISOString());

// Same local day: the clock alone is unambiguous.
assert.equal(model.formatReset(localReset(2026, 8, 14, 22, 0), at(2026, 8, 14, 8, 0)),
  'Resets in 14h 0m · 22:00');
// Both fields zero-padded.
assert.equal(model.formatReset(localReset(2026, 8, 14, 9, 5), at(2026, 8, 14, 8, 0)),
  'Resets in 1h 5m · 09:05');
// Under 24h but past midnight: the date is what stops "03:00" reading as a
// time that already went by this morning.
assert.equal(model.formatReset(localReset(2026, 8, 15, 3, 0), at(2026, 8, 14, 20, 0)),
  'Resets in 7h 0m · Aug 15 03:00');
// Long windows carry the date too.
assert.equal(model.formatReset(localReset(2026, 9, 14, 14, 30), at(2026, 8, 14, 12, 0)),
  'Resets in 31d 2h · Sep 14 14:30');
// Day-of-month is not padded, matching the rest of the row's typography.
assert.equal(model.formatReset(localReset(2026, 9, 5, 14, 0), at(2026, 8, 14, 12, 0)),
  'Resets in 22d 2h · Sep 5 14:00');
assert.equal(model.formatReset('2026-08-14T12:00:00Z', Date.parse('2026-08-14T12:00:00Z')), 'Reset due');
assert.equal(model.formatReset('', Date.parse('2026-08-14T12:00:00Z')), '');
assert.equal(model.formatReset('not-a-date', Date.parse('2026-08-14T12:00:00Z')), '');
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
const copilotPrimary = model.parseSettingsSnapshot(JSON.stringify({
  schema_version: 1, primary: 'anthropic',
  primary_choices: [{id: 'anthropic', label: 'Claude'}, {id: 'copilot', label: 'GitHub Copilot'}],
  keys: []
}));
assert.equal(copilotPrimary.ok, true);
assert.equal(copilotPrimary.primary_choices[1].id, 'copilot');
assert.equal(copilotPrimary.keys.some(key => key.id === 'copilot'), false);

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
