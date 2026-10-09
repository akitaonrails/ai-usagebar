import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const source = fs.readFileSync(new URL('./Model.js', import.meta.url), 'utf8');
const model = {};
vm.createContext(model);
vm.runInContext(source, model, {filename: 'Model.js'});

{
  const fixture = fs.readFileSync(new URL('../tests/fixtures/grokbot_paced_report.json', import.meta.url), 'utf8');
  const entry = model.parseReport(fixture).entries[0];
  assert.equal(entry.id, 'grokbot');
  assert.equal(entry.sections[0].window_secs, 864000);
  assert.equal(model.metricDetail(entry.sections[0]), '50% elapsed · 20pts ahead');
  assert.equal(model.metricMatchesWindow(entry.sections[0], 'weekly'), true);
}

{
  const fixture = fs.readFileSync(new URL('../tests/fixtures/cursor_paced_report.json', import.meta.url), 'utf8');
  const entry = model.parseReport(fixture).entries[0];
  assert.equal(entry.id, 'cursor');
  assert.equal(entry.sections[0].window_secs, 864000);
  assert.equal(entry.sections[1].window_secs, 864000);
  assert.equal(model.metricDetail(entry.sections[0]), 'Auto + Composer · 50% elapsed · 20pts ahead');
  assert.equal(model.metricDetail(entry.sections[1]), 'Named / API models · on-demand off · 50% elapsed · 20pts under');
}

const i18n = {};
vm.createContext(i18n);
vm.runInContext(fs.readFileSync(new URL('./I18n.js', import.meta.url), 'utf8'), i18n, {
  filename: 'I18n.js',
});

// Catalogs must stay in lockstep: a key in English with no locale sibling falls
// back silently to EN in the panel, which is how half-translated UI ships.
// Arrays created inside vm.runInContext live in another realm; copy out
// before deepEqual so Node's strict comparator accepts them.
assert.deepEqual(Array.from(i18n.SUPPORTED), ['en', 'ru', 'pt-BR', 'ko', 'es']);
const enKeys = Object.keys(i18n.MESSAGES.en).sort();
for (const locale of Array.from(i18n.SUPPORTED)) {
  assert.deepEqual(Object.keys(i18n.MESSAGES[locale]).sort(), enKeys, `${locale} catalog keys`);
  for (const key of enKeys) {
    const value = i18n.MESSAGES[locale][key];
    assert.equal(typeof value, 'string', `${locale}.${key} type`);
    assert.ok(value.trim().length > 0, `${locale}.${key} empty`);
  }
  assert.equal(i18n.MONTHS[locale].length, 12, `${locale} months`);
}

// LABELS drifted where MESSAGES could not: the check here used to assert that
// a single key existed, so ko shipped without "Gemini" and "Claude & GPT OSS"
// and fell back to English for them (#381). Same contract as MESSAGES now.
const enLabelKeys = Object.keys(i18n.LABELS.en).sort();
for (const locale of Array.from(i18n.SUPPORTED)) {
  assert.deepEqual(
    Object.keys(i18n.LABELS[locale]).sort(),
    enLabelKeys,
    `${locale} LABELS keys`,
  );
  for (const key of enLabelKeys) {
    const value = i18n.LABELS[locale][key];
    assert.equal(typeof value, 'string', `${locale}.LABELS.${key} type`);
    assert.ok(value.trim().length > 0, `${locale}.LABELS.${key} empty`);
  }
}

assert.equal(i18n.normalizeLocaleTag('auto'), 'auto');
assert.equal(i18n.normalizeLocaleTag(''), 'auto');
assert.equal(i18n.normalizeLocaleTag('en_US.UTF-8'), 'en');
assert.equal(i18n.normalizeLocaleTag('ru_RU'), 'ru');
assert.equal(i18n.normalizeLocaleTag('pt-BR'), 'pt-BR');
assert.equal(i18n.normalizeLocaleTag('pt_BR.UTF-8'), 'pt-BR');
assert.equal(i18n.normalizeLocaleTag('pt_PT'), 'pt-BR');
assert.equal(i18n.normalizeLocaleTag('ko_KR.UTF-8'), 'ko');
assert.equal(i18n.normalizeLocaleTag('es_MX.UTF-8'), 'es');
assert.equal(i18n.normalizeLocaleTag('es-419'), 'es');
assert.equal(i18n.normalizeLocaleTag('de_DE'), '');

assert.equal(i18n.resolveLocale('auto', 'ru_RU.UTF-8'), 'ru');
assert.equal(i18n.resolveLocale('auto', 'pt_BR'), 'pt-BR');
assert.equal(i18n.resolveLocale('auto', 'de_DE'), 'en');
assert.equal(i18n.resolveLocale('pt-BR', 'en_US'), 'pt-BR');
assert.equal(i18n.resolveLocale('ru', 'en_US'), 'ru');
assert.equal(i18n.resolveLocale('auto', 'ko_KR.UTF-8'), 'ko');
assert.equal(i18n.resolveLocale('auto', 'es_AR.UTF-8'), 'es');
assert.equal(i18n.resolveLocale('es', 'en_US'), 'es');

assert.equal(i18n.t('en', 'section.language'), 'LANGUAGE');
assert.equal(i18n.t('ru', 'section.language'), 'ЯЗЫК');
assert.equal(i18n.t('pt-BR', 'section.language'), 'IDIOMA');
assert.equal(i18n.t('ko', 'section.language'), '언어');
assert.equal(i18n.displayLabel('ko', 'On-Demand'), '온디맨드');
assert.equal(i18n.tipPoolLine('ko', 'other', 42), 'Cursor 기타 모델 · 42%');
assert.equal(i18n.t('es', 'section.language'), 'IDIOMA');
assert.equal(i18n.displayLabel('es', 'On-Demand'), 'Bajo demanda');
assert.equal(i18n.tipPoolLine('es', 'other', 42), 'Otros modelos de Cursor · 42%');
assert.equal(
  i18n.t('en', 'language.help'),
  'Language for the panel and settings. Auto matches your system language.',
);
assert.doesNotMatch(i18n.t('en', 'language.help'), /\{system\}|ru_RU|en_US/);
assert.doesNotMatch(i18n.t('ru', 'language.help'), /\{system\}|ru_RU/);
assert.doesNotMatch(i18n.t('pt-BR', 'language.help'), /\{system\}|pt_BR/);
assert.equal(i18n.t('pt-BR', 'status.refresh_failed', {error: 'boom'}),
  'Falha na atualização; mostrando o relatório anterior. boom');
assert.equal(i18n.displayLabel('ru', 'Other Models'), 'Другие модели');
assert.equal(i18n.displayLabel('pt-BR', 'On-Demand'), 'Sob demanda');
assert.equal(i18n.displayLabel('ru', 'On-Demand'), 'По запросу');
assert.equal(i18n.displayLabel('ru', 'Resets'), 'Сброс');
assert.equal(i18n.displayLabel('en', 'Unknown Metric'), 'Unknown Metric');
assert.equal(
  i18n.displayDetail('ru', 'Named / API models · on-demand off'),
  'Именованные / API-модели · on-demand выкл.',
);
assert.equal(i18n.displayDetail('ru', 'Auto + Composer'), 'Auto + Composer');
assert.equal(
  i18n.displayDetail('pt-BR', '$1.25 of $5.00 used (25%)'),
  '$1.25 de $5.00 usados (25%)',
);
assert.equal(i18n.displaySecretLabel('ru', 'API key'), 'API-ключ');
assert.equal(
  i18n.displayNote('ru', 'admin key — monthly spend'),
  'admin-ключ — месячные траты',
);
assert.equal(
  i18n.displayNote('ru', 'management key, not the inference key'),
  'management-ключ, не inference',
);
assert.equal(
  i18n.displayNote('ru', 'billing balance and monthly spend'),
  'баланс и месячные траты',
);
assert.equal(i18n.displayNote('en', 'ollama.com/settings/keys'), 'ollama.com/settings/keys');
assert.equal(
  i18n.t('ru', 'hero.settings_detail'),
  'Пока вы не нажмёте «Сохранить», ничего не изменится.',
);

const resetAt = '2026-08-14T12:00:00';
const nowMs = Date.parse('2026-08-14T08:00:00');
assert.match(i18n.formatReset(resetAt, nowMs, 'en'), /^Resets in /);
// A reset on another day carries the date; each catalog orders and suffixes it.
const nextDayReset = '2026-10-03T14:00:00';
const dayBeforeMs = Date.parse('2026-10-02T12:00:00');
assert.match(i18n.formatReset(nextDayReset, dayBeforeMs, 'en'), / · Oct 3 14:00$/);
assert.match(i18n.formatReset(nextDayReset, dayBeforeMs, 'pt-BR'), / · out 3 14:00$/);
assert.match(i18n.formatReset(nextDayReset, dayBeforeMs, 'ko'), /^1일 2시간 후 초기화 · 10월 3일 14:00$/);
assert.match(i18n.formatReset(nextDayReset, dayBeforeMs, 'es'), /^Se reinicia en 1d 2h · 3 oct 14:00$/);
assert.match(i18n.formatReset(resetAt, nowMs, 'ru'), /^Сброс через /);
assert.match(i18n.formatReset(resetAt, nowMs, 'pt-BR'), /^Redefine em /);
assert.equal(i18n.formatUpdated('', nowMs, 'pt-BR'), i18n.t('pt-BR', 'updated.unavailable'));
assert.equal(i18n.formatUpdated(new Date(nowMs - 30_000).toISOString(), nowMs, 'ru'),
  i18n.t('ru', 'updated.just_now'));
assert.equal(i18n.tipPoolLine('pt-BR', 'other', 42), 'Outros modelos Cursor · 42%');

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
assert.equal(manifest.barWidget.defaults.colorCodeUsage, false);
const colorCodeUsageSchema = manifest.barWidget.schema.find(row => row.key === 'colorCodeUsage');
assert.equal(colorCodeUsageSchema.type, 'boolean');
assert.equal(colorCodeUsageSchema.defaultValue, false);
assert.equal(manifest.barWidget.defaults.uiLocale, 'auto');
const uiLocaleSchema = manifest.barWidget.schema.find(row => row.key === 'uiLocale');
assert.equal(uiLocaleSchema.type, 'enum');
assert.deepEqual(uiLocaleSchema.options, ['auto', 'en', 'pt-BR', 'ru', 'ko', 'es']);
assert.equal(uiLocaleSchema.defaultValue, 'auto');
// Pinned window is opt-in display-only: existing shell.json entries without
// the key keep the historical highest-percent label.
assert.equal(manifest.barWidget.defaults.barWindow, 'auto');
const barWindowSchema = manifest.barWidget.schema.find(row => row.key === 'barWindow');
assert.equal(barWindowSchema.type, 'enum');
assert.deepEqual(barWindowSchema.options, ['auto', 'session', 'weekly', 'monthly']);
assert.equal(barWindowSchema.defaultValue, 'auto');
for (const key of [
  'showCursorModels', 'showCursorOther', 'showCursorOnDemand', 'showCursorCredits',
  'showAntigravityGemini', 'showAntigravityClaudeGpt'
]) {
  assert.equal(manifest.barWidget.defaults[key], true);
  const row = manifest.barWidget.schema.find(item => item.key === key);
  assert.equal(row.type, 'boolean');
  assert.equal(row.defaultValue, true);
}
// The normalizer must accept every option the manifest offers, or the
// dropdown would write a value the panel silently ignores.
for (const option of barWindowSchema.options)
  assert.equal(model.normalizeBarWindow(option), option);

const barWidgetSource = fs.readFileSync(new URL('./BarWidget.qml', import.meta.url), 'utf8');
assert.match(barWidgetSource, /^BarWidget\s*\{/m);
for (const method of ['open', 'close', 'toggle', 'closeForPopoutSwitch'])
  assert.match(barWidgetSource, new RegExp(`function\\s+${method}\\s*\\(`));
assert.match(barWidgetSource, /source:\s*Qt\.resolvedUrl\("Panel\.qml"\)/);
assert.match(barWidgetSource, /target\.anchorItem\s*=\s*button/);
assert.match(barWidgetSource, /target\.hostWidget\s*=\s*root/);
assert.match(barWidgetSource, /buttonCode\s*===\s*Qt\.RightButton\)\s*root\.launchDashboard\(\)/);
// The bar presses a slot's widget with no coordinates, so each chip registers
// as its own click target and the bar picks the one under the pointer; a lone
// chip leaves the button as the only target and its press toggles as before.
assert.match(barWidgetSource, /function\s+chipItems\s*\(/);
assert.match(barWidgetSource, /function\s+syncChipTargets\s*\(/);
assert.match(barWidgetSource, /onModelChanged:\s*Qt\.callLater\(root\.syncChipTargets\)/);
assert.match(barWidgetSource, /if\s*\(chips\.length\s*<=\s*1\)\s*return/);
// The registered target is the chip's whole column of the slot: the bar
// hit-tests it by rect, so the glyph-height delegate alone left the padding
// above and below it to the button as well as the gaps beside it.
assert.match(barWidgetSource, /Model\.chipHitGaps\(index,\s*chipRepeater\.count,\s*Style\.space\(10\),\s*Style\.spaceReal\(17\)\s*\/\s*2\)/);
// The button's edge padding lives in the outer columns, so it must not be
// added to the widget's width a second time.
assert.match(barWidgetSource, /fixedWidth:\s*root\.bar\s*&&\s*root\.bar\.vertical\s*\?\s*-1\s*:\s*chipRow\.implicitWidth\n/);
assert.match(barWidgetSource, /height:\s*button\.height/);
assert.match(barWidgetSource, /width:\s*chipContent\.implicitWidth\s*\+\s*hitGaps\.left\s*\+\s*hitGaps\.right/);
assert.match(barWidgetSource, /x:\s*chipHit\.hitGaps\.left/);
// While the panel is open the chips it is not showing step back, so the bar
// says which entry the panel belongs to. The placeholders for a lone, vertical
// or empty bar carry no id, so they never dim, and the click column above the
// content keeps full opacity.
assert.match(barWidgetSource, /opacity:\s*root\.opened\s*&&\s*root\.panelItem\s*&&\s*modelData\.id\s*\n\s*&&\s*modelData\.id\s*!==\s*root\.panelItem\.selectedEntryId\s*\?\s*0\.45\s*:\s*1/);
assert.match(barWidgetSource, /Behavior on opacity\s*\{\s*\n\s*NumberAnimation\s*\{\s*duration:\s*140/);
// The chip row's own spacing is the whole gap between the brand mark and its
// value; a spacer item there gets that spacing on both sides, so the icon sits
// nearer the previous chip's value than its own.
assert.match(barWidgetSource, /id:\s*chipContent[\s\S]*?spacing:\s*Style\.space\(4\)/);
assert.doesNotMatch(barWidgetSource, /BrandMark\s*\{[^}]*\}\s*\n\s*Item\s*\{/);
assert.match(barWidgetSource, /function\s+triggerPress\s*\(buttonCode\)/);
assert.match(barWidgetSource, /root\.panelItem\.openEntry\(chipHit\.chip\.id\s*\|\|\s*""\)/);
// Classic alarm chrome when colour-coding is off; RAG colours replace it when on.
assert.match(barWidgetSource, /active:\s*!root\.colorCodeUsage\s*&&\s*root\.alarming/);
assert.match(barWidgetSource, /text:\s*root\.alarming\s*\?\s*"󰅙"\s*:\s*"󰚩"/);
assert.doesNotMatch(barWidgetSource, /\bIpcHandler\s*\{/);

const panelSource = fs.readFileSync(new URL('./Panel.qml', import.meta.url), 'utf8');
assert.match(panelSource, /^Panel\s*\{/m);
assert.match(panelSource, /property\s+var\s+anchorItem:\s*null/);
assert.match(panelSource, /property\s+var\s+hostWidget:\s*null/);
assert.match(panelSource, /SettingsView\s*\{/);
assert.match(panelSource, /function\s+openSettings\s*\(/);
assert.match(panelSource, /function\s+openEntry\s*\(/);
assert.match(panelSource, /id:\s*chip\.id/);
assert.match(panelSource, /setting\("lastSelectedEntryId",\s*""\)/);
assert.match(panelSource, /setting\("showValue",\s*true\)/);
assert.match(panelSource, /setting\("showProvider",\s*false\)/);
assert.match(panelSource, /setting\("showAll",\s*false\)/);
assert.match(panelSource, /setting\("colorCodeUsage",\s*false\)/);
assert.match(panelSource, /Model\.normalizeBarWindow\(setting\("barWindow",\s*"auto"\)\)/);
// The pin covers the bar value and its echoes (hero detail, tooltip):
// summary (bar label/chips) is pinned, while panel rows keep every pool.
// Cursor's bar urgent state follows the pools still on the chip.
assert.match(panelSource, /Model\.headline\(shapedEntry,\s*barWindow,\s*showAs\)/);
assert.match(panelSource, /Model\.headline\(item,\s*barWindow,\s*showAs\)/);
assert.match(panelSource, /Model\.isAlarming\(item\)/);
// A failed or cached refresh alone cannot activate the bar, but a report that
// never arrived has nothing else to show there.
assert.match(panelSource, /readonly property bool reportMissing:\s*loadError\s*!==\s*""\s*&&\s*entries\.length\s*===\s*0/);
assert.match(panelSource, /\(showAll\s*\?\s*shownAnyAlarming\(\)\s*:\s*entryAlarming\)[\s\S]{0,40}reportMissing/);
assert.doesNotMatch(panelSource, /chipAlarm\s*=\s*rows\[i\]\.status\s*===\s*"error"/);
assert.match(panelSource, /function shownAnyAlarming\(\)/);
assert.doesNotMatch(panelSource, /Model\.anyAlarming\(visibleEntries\)/);
assert.doesNotMatch(panelSource, /autoSummary/);
assert.match(panelSource, /function\s+setBarWindow\s*\(/);
assert.match(panelSource, /onBarWindowRequested/);
assert.match(panelSource, /showProvider\s*\?\s*Model\.providerShort\(entry\)\s*:\s*""/);
assert.match(panelSource, /Model\.barStrip\([\s\S]*?barWindow/);
assert.match(panelSource, /Model\.barChips\([\s\S]*?barWindow/);
assert.match(panelSource, /Model\.providerIcon\(entry\)/);
assert.match(panelSource, /BrandMark\s*\{/);
assert.match(panelSource, /Model\.brandIconFile\(root\.entry\)/);

// Provider tabs must wrap into additional rows instead of being clipped by
// the panel edge when more providers are configured than fit on one line.
assert.match(panelSource, /Flow\s*\{[\s\S]*?id:\s*providerList/);
assert.match(panelSource, /flow:\s*Flow\.LeftToRight/);
assert.match(panelSource, /height:\s*visible\s*\?\s*childrenRect\.height\s*:\s*0/);
assert.match(panelSource, /width:\s*implicitWidth/);
assert.doesNotMatch(panelSource, /orientation:\s*ListView\.Horizontal/);
assert.match(panelSource, /providerList\.forceLayout\(\)/);
// The scroll content keeps a hairline of slack on both sides of the
// Flickable's clip edge. The first provider tab is a bordered button, and at
// fractional device scales (a 1.25 monitor scale) Qt drops the 1px left
// border of a control that sits exactly on the clip boundary, so that tab
// rendered with three borders. (#231)
assert.match(panelSource, /Column\s*\{[\s\S]*?id:\s*column[\s\S]*?x:\s*Style\.spacing\.hairline/);
assert.match(panelSource, /width:\s*panelFlick\.width\s*-\s*Style\.spacing\.hairline\s*\*\s*2/);
// Long settings forms must remain reachable with mouse wheels and touchpads.
assert.match(panelSource, /WheelHandler\s*\{[\s\S]*?acceptedDevices:\s*PointerDevice\.Mouse\s*\|\s*PointerDevice\.TouchPad/);
const touchpadScale = /event\.pixelDelta\.y\s*\*\s*(\d+(?:\.\d+)?)/.exec(panelSource);
assert.equal(Number(touchpadScale?.[1]), 5, 'touchpad scroll covers five times the raw pixel delta');
assert.match(panelSource, /event\.angleDelta\.y\s*\/\s*120\s*\*\s*Style\.space\(/);
// The persisted choice is the source of truth on every entries change: when
// a refresh gap briefly dropped the chosen entry, syncSelection's fallback
// re-resolved to the primary and that transient selection stuck after the
// entry returned. The remembered-entry loop must run BEFORE the
// current-selection early return so the chosen entry wins once it is back.
const syncSource = panelSource.slice(
  panelSource.indexOf('function syncSelection()'),
  panelSource.indexOf('function restoreRememberedSelection'));
assert.match(syncSource, /for \(var r = 0; r < visibleEntries\.length; r\+\+\)/, 'remembered-entry loop exists');
assert.ok(
  syncSource.indexOf('for (var r = 0') < syncSource.indexOf('for (var i = 0'),
  'remembered-entry check precedes the current-selection early return'
);
// The panel brand mark colour-codes by the summary severity only when the
// toggle is on; off, it keeps the classic binary foreground vs urgent when
// the worst pool is critical.
assert.match(panelSource, /foreground:\s*root\.colorCodeUsage\s*\n\s*\?\s*root\.severityColorOf\(root\.summary\.severity\s*\|\|\s*""\)/);
assert.match(panelSource, /\(\(root\.summary\.severity\s*\|\|\s*""\)\s*===\s*"critical"\s*\n\s*\?\s*root\.urgent\s*:\s*root\.foreground\)/);
assert.doesNotMatch(panelSource, /BrandMark[\s\S]*foreground:\s*root\.alarming\s*\?/m);
const brandMarkSource = fs.readFileSync(new URL('./BrandMark.qml', import.meta.url), 'utf8');
assert.match(brandMarkSource, /icons\/" \+ root\.brand/);
assert.ok(fs.existsSync(new URL('./icons/claude.svg', import.meta.url)));
assert.ok(fs.existsSync(new URL('./icons/openai.svg', import.meta.url)));
assert.ok(fs.existsSync(new URL('./icons/grok.svg', import.meta.url)));
assert.ok(fs.existsSync(new URL('./icons/grokbot.svg', import.meta.url)));
assert.ok(fs.existsSync(new URL('./icons/copilot.svg', import.meta.url)));
assert.match(panelSource, /function\s+persistSelection\s*\(/);
assert.match(panelSource, /Model\.settingsWithOverrides\(root\.settings,\s*root\.moduleName,\s*values\)/);
assert.match(panelSource, /bar\.shell\.updateEntryInline\(root\.moduleName,\s*entry\)/);
assert.match(panelSource, /persistSelection\(selectedEntryId\)/);
assert.match(panelSource, /Model\.barLabel\(/);
// Cursor and Antigravity pool switches filter the bar chip, tooltip and panel.
assert.match(panelSource, /Model\.panelEntry\([\s\S]*?cursorPoolFlags\(\),\s*\n?\s*antigravityPoolFlags\(\)/);
assert.doesNotMatch(panelSource, /filterCursorSections/);
assert.match(panelSource, /cursorDualHeadline\(item,\s*cursorPoolFlags\(\),\s*showAs\)/);
assert.match(panelSource, /antigravityDualHeadline\(item,\s*antigravityPoolFlags\(\),\s*showAs,\s*barWindow\)/);
assert.match(panelSource, /function creditGrantBits\(item\)/);
assert.match(panelSource, /has\.credits \|\| creditGrantBits\(entry\)\.length > 0/);
assert.match(panelSource, /return withCreditGrants\(pools, item\)/);
assert.match(panelSource, /function panelHeadline\(item\) \{[\s\S]*?providerDualHeadline\(item\)/);

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
assert.match(settingsViewSource, /root\.tr\("toggle\.show_value"\)/);
assert.match(settingsViewSource, /signal\s+showProviderRequested\(bool\s+enabled\)/);
assert.match(settingsViewSource, /root\.tr\("toggle\.show_provider"\)/);
assert.match(settingsViewSource, /signal\s+showAllRequested\(bool\s+enabled\)/);
assert.match(settingsViewSource, /root\.tr\("toggle\.show_all"\)/);
assert.match(settingsViewSource, /signal\s+colorCodeUsageRequested\(bool\s+enabled\)/);
assert.match(settingsViewSource, /signal\s+uiLocaleRequested\(string\s+value\)/);
assert.match(settingsViewSource, /root\.tr\("toggle\.color_code"\)/);
assert.match(settingsViewSource, /signal\s+barWindowRequested\(string\s+value\)/);
assert.match(settingsViewSource, /text:\s*root\.tr\("section\.bar_window"\)/);
assert.match(settingsViewSource, /value:\s*"auto",\s*label:\s*root\.tr\("bar_window\.auto"\)/);
assert.match(settingsViewSource, /value:\s*"session",\s*label:\s*root\.tr\("bar_window\.session"\)/);
assert.match(settingsViewSource, /value:\s*"weekly",\s*label:\s*root\.tr\("bar_window\.weekly"\)/);
assert.match(settingsViewSource, /value:\s*"monthly",\s*label:\s*root\.tr\("bar_window\.monthly"\)/);
assert.match(settingsViewSource, /import\s+"I18n\.js"\s+as\s+I18n/);
assert.match(settingsViewSource, /text:\s*root\.tr\("section\.language"\)/);
assert.match(settingsViewSource, /text:\s*root\.tr\("language\.help"\)/);
assert.doesNotMatch(settingsViewSource, /language\.help".*system|Qt\.locale\(\)\.name/);
assert.match(settingsViewSource, /value:\s*"auto",\s*label:\s*root\.tr\("language\.auto"\)/);
assert.match(settingsViewSource, /value:\s*"en",\s*label:\s*root\.tr\("language\.en"\)/);
assert.match(settingsViewSource, /value:\s*"pt-BR",\s*label:\s*root\.tr\("language\.pt-BR"\)/);
assert.match(settingsViewSource, /value:\s*"ru",\s*label:\s*root\.tr\("language\.ru"\)/);
assert.match(settingsViewSource, /value:\s*"ko",\s*label:\s*root\.tr\("language\.ko"\)/);
assert.match(settingsViewSource, /value:\s*"es",\s*label:\s*root\.tr\("language\.es"\)/);
assert.match(panelSource, /import\s+"I18n\.js"\s+as\s+I18n/);
assert.match(panelSource, /setting\("uiLocale",\s*"auto"\)/);
assert.match(panelSource, /I18n\.resolveLocale\(uiLocaleSetting,\s*Qt\.locale\(\)\.name\)/);
assert.match(panelSource, /function\s+setUiLocale\s*\(/);
assert.match(panelSource, /onUiLocaleRequested/);
assert.match(panelSource, /I18n\.formatReset\(/);
assert.match(panelSource, /I18n\.formatUpdated\(/);
assert.match(panelSource, /I18n\.displayLabel\(/);
assert.match(panelSource, /I18n\.displayDetail\(/);
assert.match(panelSource, /I18n\.tipPoolLine\(/);
// Settings hero: long copy wraps under the hero, not inside the detail pill.
assert.match(panelSource, /detail:\s*root\.settingsOpen\s*\?\s*""/);
assert.match(panelSource, /root\.tr\("hero\.settings_detail"\)/);
assert.match(panelSource, /I18n\.displayLabel\(root\.uiLocale,\s*detailRow\.row\.label\)/);
assert.match(settingsViewSource, /I18n\.displaySecretLabel\(/);
assert.match(settingsViewSource, /I18n\.displayNote\(/);
assert.match(settingsViewSource, /wrapMode:\s*Text\.WordWrap/);
// Env identifier stays on its own elided line; role+note wrap on the next.
assert.match(
  settingsViewSource,
  /modelData\.environment[\s\S]*?elide:\s*Text\.ElideRight[\s\S]*?displaySecretLabel[\s\S]*?displayNote[\s\S]*?wrapMode:\s*Text\.WordWrap/s,
);
assert.doesNotMatch(
  settingsViewSource,
  /parts\.push\(root\.safe\(keyCard\.modelData\.environment\)\)[\s\S]*parts\.push\(root\.safe\(keyCard\.modelData\.note\)\)/,
);
assert.match(panelSource, /barWindow:\s*root\.barWindow/);
assert.match(panelSource, /onShowAllRequested/);
assert.match(panelSource, /onShowProviderRequested/);
assert.match(settingsViewSource, /root\.tr\("auth\.nous"\)/);
assert.match(settingsViewSource, /root\.tr\("auth\.copilot"\)/);
assert.match(settingsViewSource, /root\.tr\("status\.copilot_login"\)/);
assert.match(settingsViewSource, /model:\s*root\.snapshot\.keys/);
// Provider on/off switches (#244): the section lists the snapshot's vendors
// and routes every change through the same stdin patch as the keys.
assert.match(settingsViewSource, /text:\s*root\.tr\("section\.providers"\)/);
assert.match(settingsViewSource, /model:\s*root\.snapshot\.vendors/);
assert.match(settingsViewSource, /function\s+collectVendorToggles\s*\(/);
assert.match(settingsViewSource, /function\s+setVendorOverride\s*\(/);
assert.match(
  settingsViewSource,
  /Model\.buildSettingsPatch\(selectedPrimary,\s*collectChanges\(\),\s*collectVendorToggles\(\)\)/
);
assert.match(settingsViewSource, /root\.tr\("credentials\.paste"/);
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
    brand: 'anthropic',
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
assert.equal(parsed.entries[0].brand, 'anthropic');
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
assert.equal(model.brandIconFile({id: 'grokbot'}), 'grokbot.svg');
assert.equal(model.brandIconFile({id: 'copilot'}), 'copilot.svg');
assert.equal(model.brandIconFile({id: 'devin'}), 'devin.svg');
assert.equal(model.brandIconFile({id: 'devin@work'}), 'devin.svg');
assert.equal(model.brandIconFile({id: 'custom:devin', brand: 'devin'}), 'devin.svg');
const devinMark = fs.readFileSync(new URL('./icons/devin.svg', import.meta.url), 'utf8');
assert.match(devinMark, /viewBox="0 0 24 24"/);
assert.match(devinMark, /<path\s/);
// BrandMark's colorization uses the source luminance. Match the white
// monochrome artwork contract so dark themes can tint Devin like Codex.
assert.match(devinMark, /<svg\b[^>]*\bfill="#ffffff"/);
assert.doesNotMatch(devinMark, /\bfill="(?:#000(?:000)?|black)"/i);
assert.doesNotMatch(devinMark, /<(?:script|image|foreignObject|use)\b|\bhref\s*=|\bon\w+\s*=/i);
assert.equal(model.brandIconFile({id: 'zai'}), 'zhipu.svg');
assert.equal(model.brandIconFile({id: 'kimi'}), 'kimi.svg');
assert.equal(model.brandIconFile({id: 'opencode-go'}), 'opencode.svg');
assert.equal(model.brandIconFile({id: 'ollama'}), 'ollama.svg');
assert.equal(model.brandIconFile({id: 'ollama@work'}), 'ollama.svg');
assert.equal(model.brandIconFile({id: 'custom:ollama', brand: 'ollama'}), 'ollama.svg');
const ollamaMark = fs.readFileSync(new URL('./icons/ollama.svg', import.meta.url), 'utf8');
assert.match(ollamaMark, /viewBox="0 0 24 24"/);
assert.match(ollamaMark, /\bwidth="24"/);
assert.match(ollamaMark, /\bheight="24"/);
assert.match(ollamaMark, /<path\s/);
// White source artwork is needed for BrandMark's runtime colorization.
assert.match(ollamaMark, /<svg\b[^>]*\bfill="#ffffff"/);
assert.doesNotMatch(ollamaMark, /currentColor|<(?:script|image|foreignObject|use)\b|\bhref\s*=|\bon\w+\s*=/i);
assert.equal(model.brandIconFile({id: 'lyceum'}), '');
assert.equal(model.brandIconFile({id: 'commandcode'}), '');
assert.equal(model.brandIconFile({id: 'anthropic_api'}), 'anthropic.svg');
assert.equal(model.brandIconFile({id: 'grok'}), model.brandIconFile({id: 'supergrok'}));
assert.notEqual(model.brandIconFile({id: 'grokbot'}), model.brandIconFile({id: 'grok'}));

// A custom provider carries no built-in slug, so the mark comes from the
// `brand` the report relays. A second key for the same service is the same
// product and must not read as a different one.
assert.equal(model.brandIconFile({id: 'custom:oc-second', brand: 'opencode-go'}), 'opencode.svg');
assert.equal(model.brandIconFile({id: 'custom:oc-second'}), '');
// A brand the artwork does not cover degrades to the nerd-font tag rather
// than to a blank mark, and so does an older binary's report.
assert.equal(model.brandIconFile({id: 'custom:oc-second', brand: 'commandcode'}), '');
assert.equal(model.brandIconFile({id: 'anthropic', brand: undefined}), 'claude.svg');
// `brand` wins over the id: that is the whole point of declaring it.
assert.equal(model.brandIconFile({id: 'anthropic', brand: 'openai'}), 'openai.svg');

const slugs = [
  'anthropic', 'anthropic_api', 'openai', 'copilot', 'zai', 'openrouter',
  'deepseek', 'kimi', 'kilo', 'novita', 'moonshot', 'grok', 'supergrok', 'grokbot',
  'antigravity', 'cursor', 'minimax', 'kiro', 'nous', 'opencode-go', 'lyceum', 'commandcode', 'devin', 'ollama'
];
const byMark = {};
for (const slug of slugs) {
  const mark = model.brandIconFile({id: slug}) || slug;
  byMark[mark] = (byMark[mark] || []).concat(slug);
}
const sharedMarks = Object.entries(byMark).filter(([, vendors]) =>
  vendors.length > 1 && vendors.join() !== 'grok,supergrok');
assert.deepEqual(sharedMarks, []);
const currentColorMarks = Array.from(fs.readdirSync(new URL('./icons', import.meta.url)))
  .filter(file => file.endsWith('.svg'))
  .map(file => fs.readFileSync(new URL('./icons/' + file, import.meta.url), 'utf8'))
  .filter(source => source.includes('currentColor'));
assert.equal(currentColorMarks.length, 0);
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
// Every chip names the entry behind it, in the order the bar draws them; the
// placeholders for a lone, vertical or empty bar have none to offer.
assert.equal(strip.map(chip => chip.id).join(','), 'anthropic@work,openai');
assert.equal(one[0].id, 'openai');
// The bar resolves a slot press against each registered target's own rect, so a
// chip's target is its whole column of the slot: the padding above and below
// the glyph, and half of every gap beside it. Before this, a press on that
// padding fell through to the button and toggled the entry already selected.
// The pair is copied out of the vm realm, whose objects fail a strict compare.
// The outer columns also own the button's edge padding: before, the first and
// last chips stopped at their glyph and a press at either end of the widget
// reached the button.
const chipGaps = (index, count) => {
  const value = model.chipHitGaps(index, count, 10, 8.5);
  return {left: value.left, right: value.right};
};
assert.deepEqual(chipGaps(0, 3), {left: 8.5, right: 5});
assert.deepEqual(chipGaps(1, 3), {left: 5, right: 5});
assert.deepEqual(chipGaps(2, 3), {left: 5, right: 8.5});
assert.deepEqual(chipGaps(0, 2), {left: 8.5, right: 5});
assert.deepEqual(chipGaps(1, 2), {left: 5, right: 8.5});
// A lone chip is not a target, but it still carries the edge padding the
// button's width no longer adds.
assert.deepEqual(chipGaps(0, 1), {left: 8.5, right: 8.5});
// Two half-gaps replace each plain spacing and the edges move inside the outer
// columns, so the columns add up to the widget's old padded width.
assert.equal([0, 1, 2].map(index => {
  const gaps = chipGaps(index, 3);
  return 40 + gaps.left + gaps.right;
}).reduce((sum, width) => sum + width, 0), 3 * 40 + 2 * 10 + 17);
assert.equal(40 + chipGaps(0, 1).left + chipGaps(0, 1).right, 40 + 17);
assert.equal(model.barChips([], null, false, true, false, false, true, false)[0].id, undefined);
assert.equal(model.barChips([], null, false, true, false, false, true, true)[0].id, undefined);
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
assert.equal(model.isAlarming(parsed.entries[0]), false); // cached, below critical
assert.equal(model.barChips(parsed.entries, parsed.entries[0], false, true, false, false, false, false)[0].alarming, false);
const failed = model.parseReport(JSON.stringify({entries: [{id: 'openai', status: 'error', error: 'Unavailable', sections: []}]})).entries[0];
assert.equal(model.isAlarming(failed), false);
assert.equal(model.barChips([failed], failed, false, true, false, false, false, false)[0].alarming, false);
assert.equal(model.isAlarming(parsed.entries[1]), true); // critical usage still alerts
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

// Theme palette: named Omarchy keys win over color1–3 aliases (#289 / #292).
const namedWins = model.parseThemePalette([
  'color1 = "#111111"',
  'red = "#f7768e"',
  'color2 = "#222222"',
  'green = "#9ece6a"',
  'color3 = "#333333"',
  'yellow = "#e0af68"',
  'orange = "#eb927b"',
].join('\n'));
assert.equal(namedWins.red, '#f7768e');
assert.equal(namedWins.green, '#9ece6a');
assert.equal(namedWins.yellow, '#e0af68');
assert.equal(namedWins.orange, '#eb927b');
const aliasOnly = model.parseThemePalette([
  'color1 = "#aa1111"',
  'color2 = "#11aa11"',
  'color3 = "#1111aa"',
].join('\n'));
assert.equal(aliasOnly.red, '#aa1111');
assert.equal(aliasOnly.green, '#11aa11');
assert.equal(aliasOnly.yellow, '#1111aa');
assert.equal(aliasOnly.orange, '#aa1111'); // orange falls back to resolved red
const reversedOrder = model.parseThemePalette([
  'red = "#f7768e"',
  'color1 = "#111111"',
].join('\n'));
assert.equal(reversedOrder.red, '#f7768e');

// severityColor maps the report contract onto the theme palette.
const palette = { green: '#g', yellow: '#y', orange: '#o', red: '#r' };
assert.equal(model.severityColor('low', palette), '#g');
assert.equal(model.severityColor('mid', palette), '#y');
assert.equal(model.severityColor('high', palette), '#o');
assert.equal(model.severityColor('critical', palette), '#r');
assert.equal(model.severityColor('', palette), '#g');
assert.equal(model.formatUpdated('2026-08-14T12:00:00Z', Date.parse('2026-08-14T12:03:00Z')), 'Updated 3m ago');
assert.equal(model.metricDetail(parsed.entries[0].sections[1]), '60% elapsed · 31pts under');

// Grouped sub-rows (SuperGrok's product slices) gain one heading row per
// group and pass everything else through untouched.
const supergrokSections = model.parseReport(JSON.stringify({entries: [{
  id: 'supergrok', error: null,
  sections: [
    {type: 'spacer'},
    {type: 'metric', label: 'Weekly usage', percent: 97, value: '97%', detail: '',
     severity: 'critical', reset_at: '2026-09-20T13:26:44Z', window_secs: 604800},
    {type: 'metric', label: 'Grok Build', percent: 94, value: '94%', detail: '',
     severity: 'low', group: 'Breakdown'},
    {type: 'metric', label: 'Grok Chat', percent: 3, value: '3%', detail: '',
     severity: 'low', group: 'Breakdown'},
    {type: 'text', label: 'Prepaid API', value: '$4.22'}
  ]
}]})).entries[0].sections;
assert.equal(supergrokSections[1].group, '');
assert.equal(supergrokSections[2].group, 'Breakdown');
// Array.from/JSON round-trips bridge the vm realm, like every other
// shape assertion in this file.
assert.deepEqual(Array.from(model.groupedSections(supergrokSections)).map(row => {
  if (row.type === 'spacer') return 'spacer';
  if (row.type === 'text' && row.value === '') return 'heading:' + row.label;
  return row.label;
}), [
  'spacer',
  'Weekly usage',      // ungrouped metric: no heading inserted
  'heading:Breakdown', // one heading before the group's first row…
  'Grok Build',
  'Grok Chat',         // …never a second one for the same group
  'Prepaid API'
]);
assert.equal(model.groupedSections(supergrokSections).filter(row =>
  row.type === 'metric' && row.group === 'Breakdown').length, 2);
assert.deepEqual(JSON.parse(JSON.stringify(model.groupedSections([{type: 'spacer'}]))),
  [{type: 'spacer'}]);
assert.equal(model.groupedSections(null).length, 0);
assert.equal(model.groupedSections('not-sections').length, 0);

// #255: the Claude entry's CLI-session rows arrive the same way — grouped
// metrics — so the panel draws them under one "Sessions" heading beneath the
// quota meters, with the health severity the report assigned.
const claudeSections = model.parseReport(JSON.stringify({entries: [{
  id: 'anthropic', error: null,
  sections: [
    {type: 'metric', label: 'Session (5h)', percent: 29, value: '29%', detail: '',
     severity: 'low', reset_at: '2026-09-25T14:20:00Z', window_secs: 18000},
    {type: 'metric', label: 'ship the release', percent: 90, value: '90%',
     detail: '180,000 / 200,000 tokens · claude-test · last active 12:34:56',
     severity: 'critical', group: 'Sessions'},
    {type: 'metric', label: 'sketch ideas', percent: 0, value: 'compacted',
     detail: 'compacted · waiting for the next response', severity: 'low', group: 'Sessions'},
    {type: 'text', label: '', value: '… and 4 more sessions'}
  ]
}]})).entries[0].sections;
assert.deepEqual(Array.from(model.groupedSections(claudeSections)).map(row => {
  if (row.type === 'text' && row.value === '') return 'heading:' + row.label;
  return row.type + ':' + row.label;
}), [
  'metric:Session (5h)',
  'heading:Sessions',
  'metric:ship the release',
  'metric:sketch ideas',
  'text:'                       // the overflow note is not a heading
]);
const sessionRow = model.groupedSections(claudeSections).find(row =>
  row.type === 'metric' && row.group === 'Sessions');
assert.equal(sessionRow.severity, 'critical');
assert.equal(sessionRow.value, '90%');
// …and they are not quota windows: a session at 90% of its context window
// must not become the Claude chip's value, its colour or the bar's alarm.
const claudeEntry = {id: 'anthropic', sections: claudeSections};
assert.equal(model.headline(claudeEntry).text, '29%');
assert.equal(model.headline(claudeEntry).severity, 'low');
assert.equal(model.isAlarming(claudeEntry), false);
// A grouped row still stands in when an entry has nothing else.
assert.equal(model.headline({id: 'x', sections: claudeSections.slice(1)}).text, '90%');

const balance = model.parseReport(JSON.stringify({entries: [{
  id: 'deepseek', error: null,
  sections: [{type: 'text', label: 'Balance', value: '$8.42'}]
}]})).entries[0];
assert.equal(model.headline(balance).text, '$8.42');
// The metric names its own headline; the label plays no part. A metric that
// says nothing is a percentage, which is what OpenRouter's "Credit balance" row
// is — the old label check put its dollar figure on the bar and hid the percent.
const metered = (headline) => model.parseReport(JSON.stringify({entries: [{
  id: 'openrouter', error: null,
  sections: [Object.assign(
    {type: 'metric', label: 'Credit balance', percent: 25, value: '$75.00', detail: ''},
    headline === undefined ? {} : {headline: headline})]
}]})).entries[0];
assert.equal(model.headline(metered(undefined)).text, '25%');
assert.equal(model.headline(metered('percent')).text, '25%');
assert.equal(model.headline(metered('value')).text, '$75.00');
// An unrecognized word is not a licence to invent a third rendering.
assert.equal(model.headline(metered('dollars')).text, '25%');
// A "value" headline with nothing to show falls back rather than blanking.
const emptyValue = model.parseReport(JSON.stringify({entries: [{
  id: 'deepseek', error: null,
  sections: [{type: 'metric', label: 'Balance', percent: 60, value: '',
              detail: '', headline: 'value'}]
}]})).entries[0];
assert.equal(model.headline(emptyValue).text, '60%');
// A percent headline on a row whose label says "balance" is drawn as a percent.
const meteredTank = model.parseReport(JSON.stringify({entries: [{
  id: 'deepseek', error: null,
  sections: [{type: 'metric', label: 'Balance', percent: 75, value: '$50.00',
              detail: '$50.00 of $200.00 left (75% used)', headline: 'percent'}]
}]})).entries[0];
assert.equal(model.headline(meteredTank).text, '75%');
assert.equal(model.headline(meteredTank).percent, 75);

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

// Provider on/off switches (#244): the snapshot carries every provider's
// enabled state, an older binary's vendor-less snapshot still parses, and the
// patch gains `vendors` only when a toggle is pending.
const vendorsRaw = JSON.stringify({
  schema_version: 1, primary: 'anthropic',
  primary_choices: [{id: 'anthropic', label: 'Claude'}],
  keys: [],
  vendors: [
    {id: 'anthropic', label: 'Claude', enabled: true},
    {id: 'grok', label: 'Grok', enabled: false},
    {id: 'opencode-go', label: 'OpenCode Go', enabled: true},
    {id: '__proto__', label: 'never', enabled: true},
    {id: 'kimi', label: 'Kimi', enabled: 'yes'}
  ]
});
const vendorSnapshot = model.parseSettingsSnapshot(vendorsRaw);
assert.equal(vendorSnapshot.ok, true);
assert.equal(vendorSnapshot.vendors.length, 4);
assert.equal(vendorSnapshot.vendors[0].id, 'anthropic');
assert.equal(vendorSnapshot.vendors[0].enabled, true);
assert.equal(vendorSnapshot.vendors[1].id, 'grok');
assert.equal(vendorSnapshot.vendors[1].enabled, false);
assert.equal(vendorSnapshot.vendors[2].label, 'OpenCode Go');
// A non-boolean enabled is treated as off, never coerced from a string.
assert.equal(vendorSnapshot.vendors[3].enabled, false);
const noVendors = model.parseSettingsSnapshot(JSON.stringify({
  schema_version: 1, primary: 'anthropic',
  primary_choices: [{id: 'anthropic', label: 'Claude'}], keys: []
}));
assert.equal(noVendors.ok, true);
assert.equal(noVendors.vendors.length, 0);

const togglePatch = model.buildSettingsPatch('', [], [
  {id: 'grok', enabled: true},
  {id: 'zai', enabled: false}
]);
assert.equal(togglePatch.ok, true);
assert.deepEqual(JSON.parse(togglePatch.payload), {
  schema_version: 1, keys: {}, vendors: {grok: true, zai: false}
});
// A save with no pending toggle omits `vendors`, so an older binary still
// accepts a display-only patch (its ApplyRequest denies unknown fields).
const noTogglePatch = model.buildSettingsPatch('anthropic', []);
assert.deepEqual(JSON.parse(noTogglePatch.payload), {
  schema_version: 1, primary: 'anthropic', keys: {}
});
assert.equal(model.buildSettingsPatch('', [{id: 'kimi', action: 'clear'}], undefined).ok, true);
assert.equal(model.buildSettingsPatch('', [], [{id: '__proto__', enabled: true}]).ok, false);
assert.equal(model.buildSettingsPatch('', [], [{id: 'grok'}, {id: 'grok', enabled: true}]).ok, false);
assert.equal(model.buildSettingsPatch('', [], [{id: 'grok', enabled: true}, {id: 'grok', enabled: false}]).ok, false);
assert.equal(model.buildSettingsPatch('', [], [{id: 'grok', enabled: 'on'}]).ok, false);
assert.equal(model.buildSettingsPatch('', [], []).ok, false);

// Top bar window pinning: auto keeps history, session/weekly/monthly pin one
// window class, unknown pins fall back to highest instead of blanking.
assert.equal(model.normalizeBarWindow('auto'), 'auto');
assert.equal(model.normalizeBarWindow(''), 'auto');
assert.equal(model.normalizeBarWindow('  Session '), 'session');
assert.equal(model.normalizeBarWindow('5h'), 'session');
assert.equal(model.normalizeBarWindow('rolling'), 'session');
assert.equal(model.normalizeBarWindow('WEEKLY'), 'weekly');
assert.equal(model.normalizeBarWindow('7d'), 'weekly');
assert.equal(model.normalizeBarWindow('Monthly'), 'monthly');
assert.equal(model.normalizeBarWindow('bogus'), 'auto');
assert.equal(model.normalizeBarWindow(undefined), 'auto');

const twoWindow = model.parseReport(JSON.stringify({entries: [{
  id: 'openai', error: null,
  sections: [
    {type: 'metric', label: 'Codex 5h', percent: 44, value: '44%', detail: '', severity: 'low', window_secs: 18000},
    {type: 'metric', label: 'Codex weekly', percent: 59, value: '59%', detail: '', severity: 'mid', window_secs: 604800}
  ]
}]})).entries[0];
assert.equal(twoWindow.sections[0].window_secs, 18000);
assert.equal(twoWindow.sections[1].window_secs, 604800);
assert.equal(model.headline(twoWindow).text, '59%');
assert.equal(model.headline(twoWindow, 'auto').text, '59%');
assert.equal(model.headline(twoWindow, 'session').text, '44%');
assert.equal(model.headline(twoWindow, 'session').label, 'Codex 5h');
assert.equal(model.headline(twoWindow, 'weekly').text, '59%');
assert.equal(model.headline(twoWindow, 'bogus').text, '59%');
// No monthly pool: falls back to highest rather than blanking.
assert.equal(model.headline(twoWindow, 'monthly').text, '59%');

const threeWindow = model.parseReport(JSON.stringify({entries: [{
  id: 'opencode-go', error: null,
  sections: [
    {type: 'metric', label: 'Rolling (5h)', percent: 0, value: '0%', detail: '', severity: 'low', window_secs: 18000},
    {type: 'metric', label: 'Weekly (7d)', percent: 18, value: '18%', detail: '', severity: 'low', window_secs: 604800},
    {type: 'metric', label: 'Monthly', percent: 81, value: '81%', detail: '', severity: 'high'}
  ]
}]})).entries[0];
assert.equal(threeWindow.sections[2].window_secs, null);
assert.equal(model.headline(threeWindow).text, '81%');
assert.equal(model.headline(threeWindow, 'session').text, '0%');
assert.equal(model.headline(threeWindow, 'weekly').text, '18%');
assert.equal(model.headline(threeWindow, 'monthly').text, '81%');
// Label-only match when the report predates window_secs.
const legacyWeekly = model.parseReport(JSON.stringify({entries: [{
  id: 'openai', error: null,
  sections: [
    {type: 'metric', label: 'Codex 5h', percent: 10, value: '10%', detail: ''},
    {type: 'metric', label: 'Codex weekly', percent: 90, value: '90%', detail: ''}
  ]
}]})).entries[0];
assert.equal(model.headline(legacyWeekly, 'session').text, '10%');
assert.equal(model.headline(legacyWeekly, 'weekly').text, '90%');
// window_secs wins over labels: neutral labels disambiguate by size alone,
// and a present-but-different size overrules a misleading label.
const secsOnly = model.parseReport(JSON.stringify({entries: [{
  id: 'openai', error: null,
  sections: [
    {type: 'metric', label: 'Foo', percent: 44, value: '44%', detail: '', severity: 'low', window_secs: 18000},
    {type: 'metric', label: 'Bar', percent: 59, value: '59%', detail: '', severity: 'mid', window_secs: 604800}
  ]
}]})).entries[0];
assert.equal(model.headline(secsOnly, 'session').text, '44%');
assert.equal(model.headline(secsOnly, 'weekly').text, '59%');
const secsMismatch = model.parseReport(JSON.stringify({entries: [{
  id: 'openai', error: null,
  sections: [
    {type: 'metric', label: 'Codex weekly', percent: 44, value: '44%', detail: '', severity: 'low', window_secs: 18000},
    {type: 'metric', label: 'Codex 5h', percent: 59, value: '59%', detail: '', severity: 'mid', window_secs: 604800}
  ]
}]})).entries[0];
assert.equal(model.headline(secsMismatch, 'session').text, '44%');
assert.equal(model.headline(secsMismatch, 'weekly').text, '59%');
// Weekly-only response: a session pin falls back to highest.
const weeklyOnly = model.parseReport(JSON.stringify({entries: [{
  id: 'openai', error: null,
  sections: [
    {type: 'metric', label: 'Codex weekly', percent: 77, value: '77%', detail: '', severity: 'high', window_secs: 604800}
  ]
}]})).entries[0];
assert.equal(model.headline(weeklyOnly, 'session').text, '77%');
// Abbreviated monthly spend label matches the monthly pin.
const spendMo = model.parseReport(JSON.stringify({entries: [{
  id: 'anthropic_api', error: null,
  sections: [
    {type: 'metric', label: 'Spend (mo)', percent: 12, value: '$1.34 / $1000', detail: '', severity: 'low'}
  ]
}]})).entries[0];
assert.equal(model.headline(spendMo, 'monthly').text, '12%');
// Balance-only vendors ignore the pin and keep showing the balance.
assert.equal(model.headline(balance, 'session').text, '$8.42');
// Alert state always follows the highest-percent window, never the pin;
// the pin covers the bar value plus its hero/tooltip echoes.
assert.equal(model.isAlarming(twoWindow), false);
assert.equal(model.headline(legacyWeekly, 'weekly').severity, 'critical');
assert.equal(model.isAlarming(legacyWeekly), true);
const pinnedChips = model.barChips([twoWindow, threeWindow], twoWindow, true, true, false, false, false, false, 'session');
assert.deepEqual(Array.from(pinnedChips.map(chip => chip.label)), ['44%', '0%']);
assert.equal(model.barStrip([twoWindow, threeWindow], false, false, true, false, false, 'weekly'), '󰚩  59%  󰚩  18%');
// Scoped 7-day pools carry window_secs, so the weekly pin selects them
// (max among weekly candidates, not first).
const scopedWeekly = model.parseReport(JSON.stringify({entries: [{
  id: 'anthropic', error: null,
  sections: [
    {type: 'metric', label: 'Weekly (7d)', percent: 30, value: '30%', detail: '', severity: 'low', window_secs: 604800},
    {type: 'metric', label: 'Fable (7d)', percent: 88, value: '88%', detail: '', severity: 'high', window_secs: 604800}
  ]
}]})).entries[0];
assert.equal(model.headline(scopedWeekly, 'weekly').text, '88%');
// Cursor's two pools are model categories, so the bar shows both side by
// side no matter which time window is pinned. Left is Cursor Models.
const cursorLike = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor', error: null, icon: '❯',
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 80, value: '80%', detail: '', severity: 'high'},
    {type: 'metric', label: 'Other Models', percent: 20, value: '20%', detail: '', severity: 'low'}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorLike, 'weekly').text, '80% · 20%');
assert.equal(model.headline(cursorLike, 'session').text, '80% · 20%');
assert.equal(model.headline(cursorLike, 'auto').text, '80% · 20%');
assert.equal(model.headline(cursorLike).severity, 'high');
assert.equal(model.headline(cursorLike).percent, 80);
assert.equal(model.headline(cursorLike).tooltip, 'Cursor Models · 80%\nCursor Other Models · 20%');
assert.equal(model.isAlarming(cursorLike), false);
assert.equal(model.barChips([cursorLike], cursorLike, true, true, false, false, false, false)[0].label, '80% · 20%');
assert.equal(model.barChips([cursorLike], cursorLike, true, true, false, false, false, false)[0].brand, 'cursor.svg');
assert.equal(model.barStrip([cursorLike], false, false, true, false, false), '❯  80% · 20%');
const cursorNamed = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor@work', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 35, value: '35%', detail: '', severity: 'low'},
    {type: 'metric', label: 'Other Models', percent: 7, value: '7%', detail: '', severity: 'low'}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorNamed).text, '35% · 7%');
const cursorGrant = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 35, value: '35%', detail: '', severity: 'low'},
    {type: 'metric', label: 'Other Models', percent: 7, value: '7%', detail: '', severity: 'low'},
    {type: 'metric', label: 'Promo', percent: 16, value: '$21.00', detail: '$4.00 of $25.00 used (16%)', severity: 'low', headline: 'value'}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorGrant).text, '35% · 7% · 16%');
assert.equal(model.headline(cursorGrant).severity, 'low');
assert.equal(model.headline(cursorGrant).tooltip, 'Cursor Models · 35%\nCursor Other Models · 7%\nPromo · 16%');
assert.equal(model.headline(cursorGrant).tooltipRows[2].severity, 'low');
assert.equal(model.cursorPoolPresence(cursorGrant).credits, true);
assert.equal(model.cursorDualHeadline(cursorGrant, { credits: false }).text, '35% · 7%');
assert.equal(model.cursorDualHeadline(cursorGrant, { credits: false }).tooltip, 'Cursor Models · 35%\nCursor Other Models · 7%');
const grantRow = model.groupedSections(cursorGrant.sections).filter(row => row.label === 'Promo')[0];
assert.equal(grantRow.type, 'metric');
assert.equal(grantRow.value, '$21.00');
assert.equal(grantRow.percent, 16);
const cursorPrepaid = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 35, value: '35%', detail: '', severity: 'low'},
    {type: 'metric', label: 'Other Models', percent: 7, value: '7%', detail: '', severity: 'low'},
    {type: 'text', label: 'On-Demand', value: '$0.00 / $5.00'}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorPrepaid).text, '35% · 7% · 0%');
assert.equal(model.headline(cursorPrepaid).tooltip, 'Cursor Models · 35%\nCursor Other Models · 7%\nCursor On Demand · 0%');
assert.equal(model.headline(cursorPrepaid).severity, 'low');
const prepaidRow = model.groupedSections(cursorPrepaid.sections).filter(row => row.label === 'On-Demand')[0];
assert.equal(prepaidRow.type, 'metric');
assert.equal(prepaidRow.percent, 0);
assert.equal(prepaidRow.value, '$5.00');
assert.equal(prepaidRow.detail, '$0.00 of $5.00 used (0%)');
const cursorSpent = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 35, value: '35%', detail: '', severity: 'low'},
    {type: 'metric', label: 'Other Models', percent: 7, value: '7%', detail: '', severity: 'low'},
    {type: 'text', label: 'On-Demand', value: '$1.25 / $5.00'}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorSpent).text, '35% · 7% · 25%');
assert.equal(model.groupedSections(cursorSpent.sections).filter(row => row.label === 'On-Demand')[0].value, '$3.75');
assert.equal(model.groupedSections(cursorSpent.sections).filter(row => row.label === 'On-Demand')[0].percent, 25);
const cursorDemandHot = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 10, value: '10%', detail: '', severity: 'low'},
    {type: 'metric', label: 'Other Models', percent: 7, value: '7%', detail: '', severity: 'low'},
    {type: 'text', label: 'On-Demand', value: '$4.80 / $5.00'}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorDemandHot).text, '10% · 7% · 96%');
assert.equal(model.headline(cursorDemandHot).severity, 'critical');
assert.equal(model.isAlarming(cursorDemandHot), true);
// The report's cents win over the formatted value, including a value that
// is not money at all. Percent comes from the row; dollars left come from
// the cents.
const cursorNumeric = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 35, value: '35%', detail: '', severity: 'low'},
    {type: 'metric', label: 'Other Models', percent: 7, value: '7%', detail: '', severity: 'low'},
    {type: 'text', label: 'On-Demand', value: 'not money', used_cents: 125, limit_cents: 500, percent: 25}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorNumeric).text, '35% · 7% · 25%');
const numericRow = model.groupedSections(cursorNumeric.sections).filter(row => row.label === 'On-Demand')[0];
assert.equal(numericRow.value, '$3.75');
assert.equal(numericRow.percent, 25);
assert.equal(numericRow.detail, '$1.25 of $5.00 used (25%)');
const cursorContradicts = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 35, value: '35%', detail: '', severity: 'low'},
    {type: 'metric', label: 'Other Models', percent: 7, value: '7%', detail: '', severity: 'low'},
    {type: 'text', label: 'On-Demand', value: '$9.00 / $10.00', used_cents: 0, limit_cents: 500, percent: 0}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorContradicts).text, '35% · 7% · 0%');
assert.equal(model.groupedSections(cursorContradicts.sections).filter(row => row.label === 'On-Demand')[0].value, '$5.00');
// Cents without a percent still meter. A fractional cent is not minor units
// and falls back to the formatted value.
const cursorCentsOnly = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 10, value: '10%', detail: '', severity: 'low'},
    {type: 'metric', label: 'Other Models', percent: 7, value: '7%', detail: '', severity: 'low'},
    {type: 'text', label: 'On-Demand', value: 'not money', used_cents: 480, limit_cents: 500}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorCentsOnly).text, '10% · 7% · 96%');
const cursorFractional = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 10, value: '10%', detail: '', severity: 'low'},
    {type: 'metric', label: 'Other Models', percent: 7, value: '7%', detail: '', severity: 'low'},
    {type: 'text', label: 'On-Demand', value: '$4.80 / $5.00', used_cents: 480.5, limit_cents: 500, percent: 96}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorFractional).text, '10% · 7% · 96%');
assert.equal(model.groupedSections(cursorFractional.sections).filter(row => row.label === 'On-Demand')[0].value, '$0.20');
const cursorOver = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 10, value: '10%', detail: '', severity: 'low'},
    {type: 'metric', label: 'Other Models', percent: 7, value: '7%', detail: '', severity: 'low'},
    {type: 'text', label: 'On-Demand', value: 'not money', used_cents: 600, limit_cents: 500, percent: 120}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorOver).text, '10% · 7% · 120%');
assert.equal(model.headline(cursorOver).severity, 'critical');
assert.equal(model.groupedSections(cursorOver.sections).filter(row => row.label === 'On-Demand')[0].value, '$0.00');
assert.equal(model.groupedSections(cursorOver.sections).filter(row => row.label === 'On-Demand')[0].detail, '$6.00 of $5.00 used (120%)');
const hideDemand = { models: true, other: true, demand: false };
const hideModels = { models: false, other: true, demand: true };
assert.equal(model.cursorDualHeadline(cursorPrepaid, hideDemand).text, '35% · 7%');
assert.equal(model.cursorDualHeadline(cursorPrepaid, hideDemand).tooltip, 'Cursor Models · 35%\nCursor Other Models · 7%');
assert.equal(model.cursorDualHeadline(cursorPrepaid, hideModels).text, '7% · 0%');
assert.equal(model.cursorDualHeadline(cursorPrepaid, hideModels).tooltip, 'Cursor Other Models · 7%\nCursor On Demand · 0%');
assert.equal(model.cursorDualHeadline(cursorPrepaid, { models: false, other: false, demand: true }).text, '0%');
assert.equal(model.cursorDualHeadline(cursorDemandHot, { models: false, other: false, demand: false }).text, '10%');
assert.equal(model.cursorDualHeadline(cursorDemandHot, { models: true, other: true, demand: false }).severity, 'low');
const viaDemandFirst = model.toggleCursorPool(model.toggleCursorPool({ models: true, other: true, demand: true }, 'demand'), 'models');
const viaModelsFirst = model.toggleCursorPool(model.toggleCursorPool({ models: true, other: true, demand: true }, 'models'), 'demand');
assert.equal(viaDemandFirst.models, viaModelsFirst.models);
assert.equal(viaDemandFirst.other, viaModelsFirst.other);
assert.equal(viaDemandFirst.demand, viaModelsFirst.demand);
assert.equal(viaDemandFirst.models, false);
assert.equal(viaDemandFirst.other, true);
assert.equal(viaDemandFirst.demand, false);
assert.equal(model.cursorDualHeadline(cursorPrepaid, viaDemandFirst).text, '7%');
const keptLast = model.toggleCursorPool({ models: false, other: false, demand: true, credits: false }, 'demand');
assert.equal(keptLast.models, false);
assert.equal(keptLast.other, false);
assert.equal(keptLast.demand, true);
// On-demand with no prepaid row cannot be the pool that keeps the bar alive.
const demandOnly = { models: false, other: false, demand: true };
assert.equal(model.cursorPoolPresence(cursorLike).demand, false);
assert.equal(model.cursorPoolPresence(cursorPrepaid).demand, true);
assert.equal(model.cursorBarFlags(cursorLike, demandOnly).models, true);
assert.equal(model.cursorBarFlags(cursorLike, demandOnly).demand, false);
assert.equal(model.cursorDualHeadline(cursorLike, demandOnly).text, '80%');
assert.equal(model.cursorDualHeadline(cursorLike, demandOnly).severity, 'high');
// The quieter pool can still be the one that alarms.
const cursorApiHot = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 10, value: '10%', detail: '', severity: 'low'},
    {type: 'metric', label: 'Other Models', percent: 95, value: '95%', detail: '', severity: 'critical'}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorApiHot).text, '10% · 95%');
assert.equal(model.headline(cursorApiHot).severity, 'critical');
assert.equal(model.isAlarming(cursorApiHot), true);
assert.equal(model.cursorDualHeadline(cursorApiHot, { models: true, other: false, demand: false }).severity, 'low');
// One pool, or the same labels on another vendor, stays a single figure.
const cursorOne = model.parseReport(JSON.stringify({entries: [{
  id: 'cursor', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 80, value: '80%', detail: '', severity: 'high'}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorOne).text, '80%');
assert.equal(model.headline(cursorOne).tooltip, undefined);
const cursorLabelsElsewhere = model.parseReport(JSON.stringify({entries: [{
  id: 'custom:cursorish', error: null,
  sections: [
    {type: 'metric', label: 'Cursor Models', percent: 80, value: '80%', detail: '', severity: 'high'},
    {type: 'metric', label: 'Other Models', percent: 20, value: '20%', detail: '', severity: 'low'}
  ]
}]})).entries[0];
assert.equal(model.headline(cursorLabelsElsewhere).text, '80%');
// Buckets without any window shape (Copilot-style) fall back to highest.
const copilotLike = model.parseReport(JSON.stringify({entries: [{
  id: 'copilot', error: null,
  sections: [
    {type: 'metric', label: 'Premium requests', percent: 80, value: '80%', detail: '', severity: 'high'},
    {type: 'metric', label: 'Chat', percent: 20, value: '20%', detail: '', severity: 'low'}
  ]
}]})).entries[0];
assert.equal(model.headline(copilotLike, 'weekly').text, '80%');
assert.equal(model.headline(copilotLike, 'session').text, '80%');
// The exact labels and sizes src/tui/panels.rs emits, not synthetic names:
// Z.AI ships session/weekly plus a 30-day MCP bucket with a monthly label,
// MiniMax names its pools Text/Video and its video session 24h, and SuperGrok
// puts the period in the label with no window size at all.
const zaiReal = model.parseReport(JSON.stringify({entries: [{
  id: 'zai', error: null,
  sections: [
    {type: 'metric', label: 'Session (5h)', percent: 71, value: '71%', detail: '', severity: 'mid', window_secs: 18000},
    {type: 'metric', label: 'Weekly', percent: 12, value: '12%', detail: '', severity: 'low', window_secs: 604800},
    {type: 'metric', label: 'MCP tools (monthly)', percent: 40, value: '40%', detail: '', severity: 'low', window_secs: 2592000}
  ]
}]})).entries[0];
assert.equal(model.headline(zaiReal, 'session').text, '71%');
assert.equal(model.headline(zaiReal, 'weekly').text, '12%');
assert.equal(model.headline(zaiReal, 'monthly').text, '40%');
const minimaxReal = model.parseReport(JSON.stringify({entries: [{
  id: 'minimax', error: null,
  sections: [
    {type: 'metric', label: 'Text', percent: 20, value: '20%', detail: '', severity: 'low', window_secs: 18000},
    {type: 'metric', label: 'Text', percent: 30, value: '30%', detail: '', severity: 'low', window_secs: 604800},
    {type: 'metric', label: 'Video', percent: 90, value: '90%', detail: '', severity: 'critical', window_secs: 86400},
    {type: 'metric', label: 'Video', percent: 50, value: '50%', detail: '', severity: 'mid', window_secs: 604800}
  ]
}]})).entries[0];
assert.equal(model.headline(minimaxReal, 'auto').text, '90%');
assert.equal(model.headline(minimaxReal, 'session').text, '20%');
assert.equal(model.headline(minimaxReal, 'weekly').text, '50%');
assert.equal(model.headline(minimaxReal, 'monthly').text, '90%');
assert.equal(model.isAlarming(minimaxReal), true);
const supergrokReal = model.parseReport(JSON.stringify({entries: [{
  id: 'supergrok', error: null,
  sections: [
    {type: 'metric', label: 'Monthly Build credits', percent: 42, value: '42%', detail: '', severity: 'low'}
  ]
}]})).entries[0];
assert.equal(model.headline(supergrokReal, 'monthly').text, '42%');
assert.equal(model.headline(supergrokReal, 'session').text, '42%');
const divergent = model.parseReport(JSON.stringify({entries: [{
  id: 'openai', error: null,
  sections: [
    {type: 'metric', label: 'Codex 5h', percent: 10, value: '10%', detail: '', severity: 'low', window_secs: 18000},
    {type: 'metric', label: 'Codex weekly', percent: 95, value: '95%', detail: '', severity: 'critical', window_secs: 604800}
  ]
}]})).entries[0];
assert.equal(model.headline(divergent, 'session').text, '10%');
assert.equal(model.headline(divergent, 'session').severity, 'low');
assert.equal(model.isAlarming(divergent), true);
// Chip alert state follows highest, never the pin: pinned low label with a
// critical hidden window still alarms.
const divergentChip = model.barChips([divergent], divergent, false, true, false, false, false, false, 'session')[0];
assert.equal(divergentChip.label, '10%');
assert.equal(divergentChip.alarming, true);
assert.equal(model.normalizeBarWindow('highest'), 'auto');
assert.equal(model.normalizeBarWindow('max'), 'auto');
assert.equal(model.normalizeBarWindow('shortest'), 'session');
assert.equal(model.normalizeBarWindow('session-5h'), 'session');
assert.equal(model.normalizeBarWindow('5-hour'), 'session');
assert.equal(model.normalizeBarWindow('5hour'), 'session');
assert.equal(model.normalizeBarWindow('5hr'), 'session');
assert.equal(model.normalizeBarWindow('five-hour'), 'session');
assert.equal(model.normalizeBarWindow('week'), 'weekly');
assert.equal(model.normalizeBarWindow('7-day'), 'weekly');
assert.equal(model.normalizeBarWindow('weekly-7d'), 'weekly');
assert.equal(model.normalizeBarWindow('month'), 'monthly');
assert.equal(model.normalizeBarWindow('30d'), 'monthly');
assert.equal(model.normalizeBarWindow('monthly-cycle'), 'monthly');
assert.equal(model.normalizeBarWindow(null), 'auto');
assert.deepEqual(Array.from(pinnedChips.map(chip => chip.alarming)), [false, false]);
assert.equal(model.barStrip([twoWindow], false, false, true, false, false, 'monthly'), '󰚩  59%');
const bogusChips = model.barChips([twoWindow, threeWindow], twoWindow, true, true, false, false, false, false, 'bogus');
const autoChips = model.barChips([twoWindow, threeWindow], twoWindow, true, true, false, false, false, false);
assert.deepEqual(Array.from(bogusChips.map(chip => chip.label)), Array.from(autoChips.map(chip => chip.label)));
const secsEdge = model.parseReport(JSON.stringify({entries: [{
  id: 'openai', error: null,
  sections: [
    {type: 'metric', label: 'Zero', percent: 10, value: '10%', detail: '', severity: 'low', window_secs: 0},
    {type: 'metric', label: 'Negative', percent: 20, value: '20%', detail: '', severity: 'low', window_secs: -5},
    {type: 'metric', label: 'String', percent: 30, value: '30%', detail: '', severity: 'low', window_secs: "18000"},
    {type: 'metric', label: 'Float', percent: 40, value: '40%', detail: '', severity: 'low', window_secs: 18000.9}
  ]
}]})).entries[0];
assert.equal(secsEdge.sections[0].window_secs, null);
assert.equal(secsEdge.sections[1].window_secs, null);
assert.equal(secsEdge.sections[2].window_secs, 18000);
assert.equal(secsEdge.sections[3].window_secs, 18000);
const monthlyExclude = model.parseReport(JSON.stringify({entries: [{
id: 'openai', error: null,
sections: [
{type: 'metric', label: 'Monthly quota', percent: 90, value: '90%', detail: '', severity: 'low', window_secs: 18000},
{type: 'metric', label: 'Monthly dues', percent: 10, value: '10%', detail: '', severity: 'low'}
]
}]})).entries[0];
assert.equal(model.headline(monthlyExclude, 'monthly').text, '10%');
const sessionMax = model.parseReport(JSON.stringify({entries: [{
id: 'openai', error: null,
sections: [
{type: 'metric', label: 'A 5h', percent: 20, value: '20%', detail: '', severity: 'low', window_secs: 18000},
{type: 'metric', label: 'B 5h', percent: 70, value: '70%', detail: '', severity: 'mid', window_secs: 18000}
]
}]})).entries[0];
assert.equal(model.headline(sessionMax, 'session').text, '70%');
assert.equal(model.headline(twoWindow, '5h').text, '44%');
assert.equal(model.headline(twoWindow, 'shortest').text, '44%');
assert.equal(model.headline(secsEdge, 'session').text, '40%');
assert.equal(model.barStrip([threeWindow], false, false, true, false, false, 'bogus'), '󰚩  81%');

assert.equal(model.normalizeShowAs('left'), 'left');
assert.equal(model.normalizeShowAs(' Remaining '), 'left');
assert.equal(model.normalizeShowAs('used'), 'used');
for (const junk of [undefined, null, '', 'bogus', 42, {}]) {
  assert.equal(model.normalizeShowAs(junk), 'used', `showAs ${String(junk)} stays used`);
}
assert.equal(model.shownPercent(18, 'used'), 18);
assert.equal(model.shownPercent(18, 'left'), 82);
assert.equal(model.shownPercent(130, 'left'), 0);
assert.equal(model.shownPercent('x', 'left'), null);
assert.equal(model.percentText(18, 'left'), '82%');
assert.equal(model.percentText(18), '18%');
assert.equal(model.percentText('x', 'left'), '');

const zaiReport = (mcpPercent) => model.parseReport(JSON.stringify({entries: [{
  id: 'zai',
  name: 'Z.AI',
  sections: [
    {type: 'metric', label: 'Session', percent: 0, window_secs: 18000},
    {type: 'metric', label: 'Weekly', percent: 0, window_secs: 604800},
    {type: 'metric', label: 'MCP tools (monthly)', percent: mcpPercent}
  ]
}]})).entries[0];

{
  const zai = zaiReport(18);
  assert.equal(model.headline(zai).text, '18%');
  const shaped = model.visibleEntry(zai, ['MCP tools (monthly)']);
  assert.equal(model.headline(shaped).text, '0%');
  assert.equal(model.headline(shaped, 'auto').percent, 0);
  assert.equal(zai.sections.length, 3, 'the original entry keeps every row');
  assert.equal(shaped.sections.length, 2);
  assert.equal(model.visibleEntry(zai, []), zai, 'nothing hidden returns the entry itself');
  assert.equal(model.visibleEntry(zai, ['No such metric']), zai, 'an unknown key hides nothing');
  assert.equal(model.visibleEntry(null, ['x']), null);

  const spent = zaiReport(100);
  assert.equal(model.isAlarming(spent), true);
  assert.equal(model.isAlarming(model.visibleEntry(spent, ['MCP tools (monthly)'])), false);

  assert.equal(model.headline(model.visibleEntry(zai, ['MCP tools (monthly)']), 'monthly').text, '0%',
    'a pinned window that was hidden falls back to what is left');
  assert.equal(model.barChips([shaped], shaped, false, true, false, false, false, false, 'auto')[0].label, '0%');
}

{
  const zai = zaiReport(18);
  assert.equal(model.headline(zai, 'auto', 'left').text, '82%');
  assert.equal(model.headline(zai, 'auto', 'left').severity, 'low');
  assert.equal(model.headline(zai, 'auto', 'left').percent, 18, 'the percent stays the used share');
  const shaped = model.visibleEntry(zai, ['MCP tools (monthly)']);
  assert.equal(model.headline(shaped, 'auto', 'left').text, '100%');
  assert.equal(model.headline(zaiReport(100), 'auto', 'left').text, '0%');
  assert.equal(model.barChips([zai], zai, false, true, false, false, false, false, 'auto', 'left')[0].label, '82%');
  assert.equal(model.barStrip([zai], false, false, true, false, false, 'auto', 'left'), '󰚩  82%');
  const valued = model.parseReport(JSON.stringify({entries: [{id: 'openrouter', sections: [
    {type: 'metric', label: 'Credits', percent: 30, value: '$7.00', headline: 'value'}
  ]}]})).entries[0];
  assert.equal(model.headline(valued, 'auto', 'left').text, '$7.00', 'a value headline is not a percentage');
}

{
  const zai = zaiReport(18);
  const all = ['Session', 'Weekly', 'MCP tools (monthly)'];
  assert.equal(model.visibleEntry(zai, all), zai, 'hiding every metric is ignored, so the bar never goes blank');
  assert.equal(model.headline(model.visibleEntry(zai, all)).text, '18%');
  assert.equal(model.visibleEntry(zai, ['Session', 'Weekly']).sections.length, 1, 'one metric left is honored');
  assert.equal(model.headline(model.visibleEntry(zai, ['Session', 'Weekly'])).text, '18%');
  assert.equal(model.headline({id: 'zai', sections: [], status: 'ready'}).text, 'Ready', 'an entry that never had a meter keeps Ready');
  const withBalance = model.parseReport(JSON.stringify({entries: [{id: 'zai', sections: [
    {type: 'metric', label: 'Session', percent: 40},
    {type: 'text', label: 'Balance', value: '$3.00'}
  ]}]})).entries[0];
  assert.equal(model.visibleEntry(withBalance, ['Session']), withBalance, 'the only metric cannot be hidden even with a balance beside it');
  assert.equal(model.headline(model.visibleEntry(withBalance, ['Session'])).text, '40%');
}

{
  const zai = zaiReport(18);
  const [session, weekly, mcp] = Array.from(zai.sections);
  const keyOf = (entry, section) => model.metricKeys(entry)[Array.from(entry.sections).indexOf(section)].key;
  const can = (hidden, section) => model.canToggleMetric(zai, hidden, keyOf(zai, section));
  assert.equal(can({}, session) && can({}, weekly) && can({}, mcp), true, 'with all on, any one may be hidden');
  const two = {zai: ['Session', 'Weekly']};
  assert.equal(can(two, mcp), false, 'the last metric still on cannot be hidden');
  assert.equal(can(two, session), true, 'a hidden metric can always be shown again');
  assert.equal(can(two, weekly), true);
  assert.equal(can({zai: ['Session']}, mcp), true);
  assert.equal(can({zai: ['Session']}, weekly), true);
  assert.equal(can({'zai@other': ['Session', 'Weekly']}, mcp), true, 'another entry id does not count');
  assert.equal(model.canToggleMetric(zai, {}, ''), false);
  assert.equal(model.canToggleMetric(null, {}, keyOf(zai, session)), false);
  const single = model.parseReport(JSON.stringify({entries: [{id: 'solo', sections: [
    {type: 'metric', label: 'Only', percent: 5}
  ]}]})).entries[0];
  assert.equal(model.canToggleMetric(single, {}, keyOf(single, single.sections[0])), false, 'a lone metric has no switch to flip');
}

{
  const grok = model.parseReport(JSON.stringify({entries: [{id: 'supergrok', sections: [
    {type: 'metric', label: 'Credits', percent: 10},
    {type: 'metric', label: 'Chat', percent: 90, group: 'Breakdown'}
  ]}]})).entries[0];
  assert.equal(model.metricKeys(grok)[1].key, 'Breakdown / Chat');
  assert.equal(model.headline(grok).text, '10%');
  assert.equal(model.headline(model.visibleEntry(grok, ['Credits'])).text, '90%');
  assert.equal(model.headline(model.visibleEntry(grok, ['Chat'])).text, '10%');
  assert.equal(model.headline(model.visibleEntry(grok, ['Breakdown / Chat'])).text, '10%');
}

{
  const clean = model.normalizeHiddenMetrics(JSON.parse(`{
    "zai": ["MCP tools (monthly)", " MCP tools (monthly) ", "", 7],
    "anthropic@work": ["Weekly"],
    "empty": [],
    "broken": "Weekly",
    "__proto__": ["x"],
    "constructor": ["x"]
  }`));
  assert.deepEqual(Object.keys(clean).sort(), ['anthropic@work', 'zai']);
  assert.deepEqual(Array.from(clean.zai), ['MCP tools (monthly)', '7']);
  for (const junk of [null, undefined, 'zai', 5, ['zai'], true]) {
    assert.deepEqual(Object.keys(model.normalizeHiddenMetrics(junk)), []);
  }
  const wide = {};
  for (let i = 0; i < 100; i++) wide['p' + i] = ['k'];
  assert.equal(Object.keys(model.normalizeHiddenMetrics(wide)).length, 64);
  const tall = {zai: Array.from({length: 100}, (_, i) => 'k' + i)};
  assert.equal(model.normalizeHiddenMetrics(tall).zai.length, 32);

  assert.deepEqual(Array.from(model.hiddenKeysFor(clean, 'zai')), ['MCP tools (monthly)', '7']);
  assert.deepEqual(Array.from(model.hiddenKeysFor(clean, 'toString')), []);
  assert.deepEqual(Array.from(model.hiddenKeysFor(clean, 'anthropic')), [], 'an entry id is matched exactly');
}

{
  let hidden = model.toggleHiddenMetric({}, 'zai', 'MCP tools (monthly)');
  assert.deepEqual(Array.from(hidden.zai), ['MCP tools (monthly)']);
  hidden = model.toggleHiddenMetric(hidden, 'zai', 'Weekly');
  assert.deepEqual(Array.from(hidden.zai), ['MCP tools (monthly)', 'Weekly']);
  hidden = model.toggleHiddenMetric(hidden, 'zai', 'MCP tools (monthly)');
  assert.deepEqual(Array.from(hidden.zai), ['Weekly']);
  hidden = model.toggleHiddenMetric(hidden, 'zai', 'Weekly');
  assert.deepEqual(Object.keys(hidden), []);
  assert.deepEqual(Object.keys(model.toggleHiddenMetric({}, '', 'x')), []);
  assert.deepEqual(Object.keys(model.toggleHiddenMetric({}, 'zai', ' ')), []);
  assert.deepEqual(Object.keys(model.toggleHiddenMetric({}, '__proto__', 'x')), []);
  const before = {zai: ['Weekly']};
  model.toggleHiddenMetric(before, 'zai', 'Session');
  assert.deepEqual(before.zai, ['Weekly'], 'the input map is not mutated');
}

{
  const cursor = model.parseReport(JSON.stringify({entries: [{id: 'cursor', sections: [
    {type: 'metric', label: 'Cursor Models', percent: 35, severity: 'low'},
    {type: 'metric', label: 'Other Models', percent: 100, severity: 'critical'}
  ]}]})).entries[0];
  assert.equal(model.headline(cursor).text, '35% · 100%');
  assert.equal(model.headline(cursor, 'auto', 'left').text, '65% · 0%');
  assert.equal(model.headline(cursor, 'auto', 'left').percent, 100);
  assert.equal(model.headline(cursor, 'auto', 'left').severity, 'critical');
  assert.equal(model.cursorDualHeadline(cursor, undefined, 'left').tooltip, 'Cursor Models · 65%\nCursor Other Models · 0%');
}

{
  const view = (row, showAs) => {
    const out = model.metricValueView(row, showAs);
    return {text: out.text, left: out.left, percent: out.percent};
  };
  assert.deepEqual(view({percent: 4, value: '4%'}, 'left'), {text: '96%', left: true, percent: 96});
  assert.deepEqual(view({percent: 4, value: '4%'}, 'used'), {text: '4%', left: false, percent: null});
  assert.deepEqual(view({percent: 4, value: ''}, 'left'), {text: '96%', left: true, percent: 96});
  assert.deepEqual(view({percent: 4, value: ''}, 'used'), {text: '4%', left: false, percent: 4});
  assert.deepEqual(view({percent: 30, value: '$7.00'}, 'left'), {text: '$7.00', left: false, percent: null},
    'a value with its own information is kept');
  assert.deepEqual(view({percent: 4, value: '12 / 50 requests'}, 'left'),
    {text: '12 / 50 requests', left: false, percent: null});
  assert.deepEqual(view({percent: 4.4, value: '4.4%'}, 'used'), {text: '4.4%', left: false, percent: null},
    'the used reading keeps the report value untouched');
  assert.deepEqual(view({percent: 100, value: '100%'}, 'left'), {text: '0%', left: true, percent: 0});
  assert.deepEqual(view(null, 'left'), {text: '', left: false, percent: null});
}

{
  const zai = zaiReport(18);
  const panel = (hidden) => Array.from(model.panelEntry(zai, hidden, {}).sections.map(row => row.label));
  assert.deepEqual(panel({}), ['Session', 'Weekly', 'MCP tools (monthly)']);
  assert.deepEqual(panel({zai: ['MCP tools (monthly)']}), ['Session', 'Weekly']);
  assert.deepEqual(panel({zai: ['Session', 'Weekly']}), ['MCP tools (monthly)']);
  assert.deepEqual(panel({zai: ['Session', 'Weekly', 'MCP tools (monthly)']}), ['Session', 'Weekly', 'MCP tools (monthly)'],
    'hiding every metric is ignored');
  assert.deepEqual(panel({'zai@other': ['Session']}), ['Session', 'Weekly', 'MCP tools (monthly)']);
  assert.equal(model.panelEntry(null, {}, {}), null);
  const grok = model.parseReport(JSON.stringify({entries: [{id: 'supergrok', sections: [
    {type: 'metric', label: 'Credits', percent: 10},
    {type: 'metric', label: 'Chat', percent: 90, group: 'Breakdown'}
  ]}]})).entries[0];
  const rows = (hidden) => Array.from(model.groupedSections(model.panelEntry(grok, hidden, {}).sections).map(row => row.label));
  assert.deepEqual(rows({}), ['Credits', 'Breakdown', 'Chat']);
  assert.deepEqual(rows({supergrok: ['Breakdown / Chat']}), ['Credits'], 'a group with no row left loses its heading');
}

{
  const agy = model.parseReport(JSON.stringify({entries: [{id: 'antigravity', sections: [
    {type: 'spacer'},
    {type: 'text', label: 'Session', value: ''},
    {type: 'spacer'},
    {type: 'metric', label: 'Gemini', percent: 0, window_secs: 18000},
    {type: 'spacer'},
    {type: 'metric', label: 'Claude & GPT OSS', percent: 4, window_secs: 18000},
    {type: 'spacer'},
    {type: 'text', label: 'Weekly', value: ''},
    {type: 'spacer'},
    {type: 'metric', label: 'Gemini', percent: 6, window_secs: 604800},
    {type: 'spacer'},
    {type: 'metric', label: 'Claude & GPT OSS', percent: 2, window_secs: 604800},
    {type: 'spacer'},
    {type: 'text', label: 'Source', value: 'local'}
  ]}]})).entries[0];
  const brief = (entry) => Array.from(entry.sections.map(row => row.type === 'spacer' ? '_' : (row.type === 'text' ? 't:' : 'm:') + row.label)).join(' ');
  assert.deepEqual(Array.from(model.metricKeys(agy), item => item && item.key),
    [null, null, null, 'Session / Gemini', null, 'Session / Claude & GPT OSS', null, null, null, 'Weekly / Gemini', null, 'Weekly / Claude & GPT OSS', null, null]);
  const shown = (hidden) => brief(model.visibleEntry(agy, hidden));
  const FULL = '_ t:Session _ m:Gemini _ m:Claude & GPT OSS _ t:Weekly _ m:Gemini _ m:Claude & GPT OSS _ t:Source';
  assert.equal(shown([]), FULL);
  assert.equal(shown(['Weekly / Gemini']),
    '_ t:Session _ m:Gemini _ m:Claude & GPT OSS _ t:Weekly _ m:Claude & GPT OSS _ t:Source',
    'hiding one of two same-labelled rows hides only that one, with the spacer that led it');
  assert.equal(shown(['Session / Gemini', 'Session / Claude & GPT OSS']),
    '_ t:Weekly _ m:Gemini _ m:Claude & GPT OSS _ t:Source',
    'a heading with no metric left goes with its spacers, leaving no gap');
  assert.equal(shown(['Weekly / Gemini', 'Weekly / Claude & GPT OSS']),
    '_ t:Session _ m:Gemini _ m:Claude & GPT OSS _ t:Source', 'and the Source row stays');
  const flags = (over = {}) => Object.assign({gemini: true, third_party: true}, over);
  const poolRows = (over = {}) => brief(model.panelEntry(agy, {}, {}, flags(over)));
  assert.equal(poolRows(), FULL);
  assert.equal(poolRows({gemini: false}), '_ t:Session _ m:Claude & GPT OSS _ t:Weekly _ m:Claude & GPT OSS _ t:Source');
  assert.equal(poolRows({third_party: false}), '_ t:Session _ m:Gemini _ t:Weekly _ m:Gemini _ t:Source');
  const toggledPools = model.toggleAntigravityPool({gemini: false, third_party: false}, 'gemini');
  assert.equal(toggledPools.gemini, true);
  assert.equal(toggledPools.third_party, false);
  assert.equal(model.headline(agy).text, '6% · 4%');
  assert.equal(model.headline(agy, 'auto', 'left').text, '94% · 96%');
  assert.equal(model.headline(agy, 'session', 'used').text, '0% · 4%');
  assert.equal(model.headline(agy, 'weekly', 'used').text, '6% · 2%');
  assert.equal(model.antigravityDualHeadline(agy, flags({third_party: false}), 'used').text, '6%');
  assert.equal(model.antigravityDualHeadline(agy, flags({gemini: false}), 'used').text, '4%');
  const windowRows = (hidden) => Array.from(model.metricChoices(agy, hidden),
    row => `${row.key}|${row.label}|${row.checked}|${row.canToggle}`);
  assert.deepEqual(windowRows({}), ['window:session|Session (5h)|true|true', 'window:weekly|Weekly (7d)|true|true'],
    'Antigravity offers its windows; the pools are the panel buttons');
  const noWeekly = model.toggleMetricChoice({}, agy, 'window:weekly');
  assert.deepEqual(Array.from(noWeekly.antigravity), ['Weekly / Gemini', 'Weekly / Claude & GPT OSS']);
  assert.deepEqual(windowRows(noWeekly), ['window:session|Session (5h)|true|false', 'window:weekly|Weekly (7d)|false|true'],
    'the last window on is locked');
  assert.equal(model.canToggleMetric(agy, noWeekly, 'window:session'), false);
  assert.equal(model.canToggleMetric(agy, noWeekly, 'window:weekly'), true);
  assert.deepEqual(Object.keys(model.toggleMetricChoice(noWeekly, agy, 'window:weekly')), [], 'switching it back clears the list');
  const sessionOnly = model.panelEntry(agy, noWeekly, {}, flags());
  assert.equal(brief(sessionOnly), '_ t:Session _ m:Gemini _ m:Claude & GPT OSS _ t:Source', 'a hidden window leaves no stacked spacers');
  assert.equal(model.headline(sessionOnly).text, '0% · 4%', 'and the bar no longer reads it');
  const twice = model.parseReport(JSON.stringify({entries: [{id: 'x', sections: [
    {type: 'metric', label: 'Pool', percent: 1},
    {type: 'metric', label: 'Pool', percent: 2}
  ]}]})).entries[0];
  assert.deepEqual(Array.from(model.metricKeys(twice), item => item.key), ['Pool', 'Pool #2'], 'a repeated label is numbered');
}

{
  const partial = model.parseReport(JSON.stringify({entries: [{id: 'antigravity', sections: [
    {type: 'spacer'},
    {type: 'text', label: 'Session', value: ''},
    {type: 'metric', label: 'Gemini', percent: 10, window_secs: 18000},
    {type: 'spacer'},
    {type: 'text', label: 'Weekly', value: ''},
    {type: 'metric', label: 'Gemini', percent: 20, window_secs: 604800},
    {type: 'metric', label: 'Claude & GPT OSS', percent: 95, window_secs: 604800}
  ]}]})).entries[0];
  const noWeekly = model.toggleMetricChoice({}, partial, 'window:weekly');
  const geminiOff = {gemini: false, third_party: true};
  const shown = model.panelEntry(partial, noWeekly, {}, geminiOff);
  assert.deepEqual(Array.from(shown.sections.map(row => row.type + ':' + (row.label || ''))),
    ['spacer:', 'text:Session', 'metric:Gemini'],
    'a pool button never brings a hidden window back; the pool that is left stays');
  assert.equal(model.headline(shown).text, '10%');
  assert.equal(model.isAlarming(shown), false);
}

{
  const account = model.parseReport(JSON.stringify({entries: [{id: 'antigravity@work', sections: [
    {type: 'text', label: 'Session', value: ''},
    {type: 'metric', label: 'Gemini', percent: 1, window_secs: 18000},
    {type: 'text', label: 'Weekly', value: ''},
    {type: 'metric', label: 'Gemini', percent: 2, window_secs: 604800}
  ]}]})).entries[0];
  const next = model.toggleMetricChoice({}, account, 'window:weekly');
  assert.deepEqual(Array.from(next['antigravity@work']), ['Weekly / Gemini'], 'each account keeps its own windows');
  assert.deepEqual(Array.from(model.metricChoices(account, {'antigravity': ['Weekly / Gemini']}), row => row.checked), [true, true]);
  const stale = {'antigravity@work': ['Weekly / Gemini', 'Not a pool row']};
  assert.deepEqual(Array.from(model.metricChoices(account, stale), row => row.checked), [true, false]);
  const crowded = {'antigravity@work': Array.from({length: 31}, (_, i) => 'old ' + i)};
  assert.equal(model.metricChoices(account, crowded).every(row => row.canToggle), true,
    'keys that no row offers do not count toward the bound');
  const dense = model.parseReport(JSON.stringify({entries: [{id: 'antigravity', sections: [
    {type: 'metric', label: 'Gemini', percent: 1, window_secs: 18000},
    {type: 'metric', label: 'Gemini', percent: 2, window_secs: 604800}
  ]}]})).entries[0];
  assert.equal(model.windowOfMetric({window_secs: 604800, type: 'metric', label: 'Gemini'}, 'Session'), 'weekly',
    'a stated 7d length wins over the heading');
  assert.equal(model.windowOfMetric({type: 'metric', label: 'Gemini'}, 'Session'), 'session');
  assert.equal(model.windowOfMetric({type: 'metric', label: 'Gemini'}, ''), 'monthly');
  assert.deepEqual(Array.from(model.metricChoices(dense, {}), row => row.key), ['window:session', 'window:weekly']);
}

{
  const future = model.parseReport(JSON.stringify({entries: [{id: 'cursor', sections: [
    {type: 'metric', label: 'Cursor Models', percent: 80, window_secs: 604800},
    {type: 'metric', label: 'Other Models', percent: 30, window_secs: 604800},
    {type: 'metric', label: 'Cursor Models', percent: 10, window_secs: 2678400},
    {type: 'metric', label: 'Team credit', percent: 15}
  ]}]})).entries[0];
  assert.deepEqual(Array.from(model.metricChoices(future, {}), row => row.key + '|' + row.canToggle),
    ['window:weekly|true', 'window:monthly|true'], 'a window Cursor gains later gets its own switch');
  const hiddenWeekly = model.toggleMetricChoice({}, future, 'window:weekly');
  const rows = Array.from(model.panelEntry(future, hiddenWeekly, {}).sections.map(row => row.label + ':' + row.percent));
  assert.deepEqual(rows, ['Cursor Models:10', 'Team credit:15'], 'the grant is never part of a window row');
  const grantOnly = {cursor: ['Team credit']};
  assert.equal(model.panelEntry(future, grantOnly, {}).sections.length, 4, 'a stale grant key hides nothing Settings cannot restore');
}

{
  const real = model.parseReport(JSON.stringify({entries: [{id: 'antigravity', sections: [
    {type: 'spacer'},
    {type: 'text', label: 'Session', value: ''},
    {type: 'spacer'},
    {type: 'metric', label: 'Gemini', percent: 1, window_secs: 18000},
    {type: 'spacer'},
    {type: 'text', label: 'Weekly', value: ''},
    {type: 'spacer'},
    {type: 'metric', label: 'Gemini', percent: 2, window_secs: 604800},
    {type: 'metric', label: 'Claude & GPT OSS', percent: 3, window_secs: 604800}
  ]}]})).entries[0];
  const crowded = {antigravity: Array.from({length: 31}, (_, i) => 'old ' + i)};
  const row = (hidden) => Array.from(model.metricChoices(real, hidden), item => `${item.key}|${item.checked}|${item.canToggle}`);
  assert.deepEqual(row(crowded), ['window:session|true|true', 'window:weekly|true|true'], 'stale keys no row offers count for nothing');
  const cleaned = model.toggleMetricChoice(crowded, real, 'window:weekly');
  assert.deepEqual(Array.from(cleaned.antigravity), ['Weekly / Gemini', 'Weekly / Claude & GPT OSS'],
    'a toggle drops the stale keys, so the bound it checks is the one the row showed');
  const partial = {antigravity: ['Weekly / Gemini']};
  assert.deepEqual(row(partial), ['window:session|true|false', 'window:weekly|false|true'], 'a window hidden in part reads as off');
  assert.deepEqual(Object.keys(model.toggleMetricChoice(partial, real, 'window:weekly')), [], 'and one click restores it');
  const agyOff = {gemini: true, third_party: false};
  const painted = model.panelEntry(real, {antigravity: ['Weekly / Gemini', 'Weekly / Claude & GPT OSS']}, {}, agyOff);
  assert.equal(model.antigravityPoolPresence(model.windowEntry(real, {antigravity: ['Weekly / Gemini', 'Weekly / Claude & GPT OSS']})).third_party, false,
    'the pool buttons read the entry with the hidden window already gone');
  assert.equal(model.headline(painted).text, '1%');
}

{
  const brief = (entry) => Array.from(entry.sections.map(row => row.type === 'spacer' ? '_' : (row.type === 'text' ? 't:' : 'm:') + row.label)).join(' ');
  const claude = model.parseReport(JSON.stringify({entries: [{id: 'anthropic', sections: [
    {type: 'metric', label: 'Extra usage', percent: 5},
    {type: 'spacer'},
    {type: 'metric', label: 'proj-a', percent: 1, group: 'Sessions'},
    {type: 'metric', label: 'proj-b', percent: 2, group: 'Sessions'}
  ]}]})).entries[0];
  assert.equal(brief(model.visibleEntry(claude, ['Sessions / proj-a'])), 'm:Extra usage _ m:proj-b',
    'a spacer shared by a group stays for the row that is left');
  assert.equal(brief(model.visibleEntry(claude, ['Sessions / proj-b'])), 'm:Extra usage _ m:proj-a');
  const grok = model.parseReport(JSON.stringify({entries: [{id: 'supergrok', sections: [
    {type: 'spacer'},
    {type: 'metric', label: 'Weekly usage', percent: 5},
    {type: 'metric', label: 'Chat', percent: 1, group: 'Breakdown'}
  ]}]})).entries[0];
  assert.equal(brief(model.visibleEntry(grok, ['Weekly usage'])), '_ m:Chat');
  const lopsided = model.parseReport(JSON.stringify({entries: [{id: 'antigravity', sections: [
    {type: 'spacer'},
    {type: 'text', label: 'Session', value: ''},
    {type: 'spacer'},
    {type: 'metric', label: 'Gemini', percent: 1, window_secs: 18000},
    {type: 'spacer'},
    {type: 'text', label: 'Weekly', value: ''},
    {type: 'spacer'},
    {type: 'metric', label: 'Gemini', percent: 2, window_secs: 604800},
    {type: 'spacer'},
    {type: 'metric', label: 'Claude & GPT OSS', percent: 3, window_secs: 604800}
  ]}]})).entries[0];
  const geminiOff = model.panelEntry(lopsided, {}, {}, {gemini: false, third_party: true});
  assert.equal(brief(geminiOff), '_ t:Weekly _ m:Claude & GPT OSS', 'a window left with no pool loses its heading too');
  const noWeekly = model.toggleMetricChoice({}, lopsided, 'window:weekly');
  assert.equal(model.antigravityPoolPresence(model.windowEntry(lopsided, noWeekly)).third_party, false,
    'a pool that only lives in a hidden window gets no button');
}

{
  const zai = zaiReport(18);
  const choices = (hidden) => Array.from(model.metricChoices(zai, hidden),
    row => `${row.label}|${row.checked}|${row.canToggle}`);
  assert.deepEqual(choices({}), ['Session|true|true', 'Weekly|true|true', 'MCP tools (monthly)|true|true']);
  assert.deepEqual(choices({zai: ['Weekly']}), ['Session|true|true', 'Weekly|false|true', 'MCP tools (monthly)|true|true']);
  assert.deepEqual(choices({zai: ['Session', 'Weekly']}), ['Session|false|true', 'Weekly|false|true', 'MCP tools (monthly)|true|false'],
    'the last metric on has its switch locked');
  assert.deepEqual(Array.from(model.metricChoices(null, {})), []);
  const grouped = model.parseReport(JSON.stringify({entries: [{id: 'supergrok', sections: [
    {type: 'metric', label: 'Credits', percent: 10},
    {type: 'metric', label: 'Chat', percent: 90, group: 'Breakdown'}
  ]}]})).entries[0];
  const rows = model.metricChoices(grouped, {});
  assert.deepEqual(Array.from(rows, row => row.key), ['Credits', 'Breakdown / Chat']);
  assert.equal(rows[1].group, 'Breakdown');
  const cursor = model.parseReport(JSON.stringify({entries: [{id: 'cursor', sections: [
    {type: 'metric', label: 'Cursor Models', percent: 35},
    {type: 'metric', label: 'Other Models', percent: 100},
    {type: 'text', label: 'On-Demand', value: '$1.00 / $5.00'},
    {type: 'metric', label: 'Team credit', percent: 15},
    {type: 'metric', label: 'Spend grant', percent: 5}
  ]}]})).entries[0];
  const flags = (over) => Object.assign({models: true, other: true, demand: true, credits: true}, over);
  assert.deepEqual(Array.from(model.metricChoices(cursor, {}), row => `${row.key}|${row.checked}|${row.canToggle}`),
    ['window:monthly|true|false'], 'Cursor has one window today, so its switch is locked like a single-metric provider');
  assert.deepEqual(Object.keys(model.toggleMetricChoice({}, cursor, 'window:session')), [], 'a window Cursor lacks changes nothing');
  const kept = (over) => Array.from(model.panelEntry(cursor, {}, flags(over)).sections.map(row => row.label));
  assert.deepEqual(kept({}), ['Cursor Models', 'Other Models', 'On-Demand', 'Team credit', 'Spend grant']);
  assert.deepEqual(kept({other: false}), ['Cursor Models', 'On-Demand', 'Team credit', 'Spend grant']);
  assert.deepEqual(kept({models: false, credits: false}), ['Other Models', 'On-Demand']);
  assert.deepEqual(kept({models: false, other: false, demand: false, credits: false}), ['Cursor Models'],
    'the first pool stays when every switch is off');
  const plainDemand = model.parseReport(JSON.stringify({entries: [{id: 'cursor', sections: [
    {type: 'metric', label: 'Cursor Models', percent: 35},
    {type: 'metric', label: 'Other Models', percent: 100},
    {type: 'text', label: 'On-Demand', value: '$0.00'}
  ]}]})).entries[0];
  assert.deepEqual(Array.from(model.panelEntry(plainDemand, {}, flags({other: false})).sections.map(row => row.label)),
    ['Cursor Models', 'On-Demand'], 'an On-Demand row without a limit is not a pool, so it is never cut');
  assert.deepEqual(Array.from(model.metricChoices(plainDemand, {}), row => row.key), ['window:monthly'],
    'and it gets no switch of its own, only the window row');
}

{
  const zai = zaiReport(100);
  const shaped = model.panelEntry(zai, {zai: ['MCP tools (monthly)']}, {}, {});
  assert.equal(model.headline(shaped).text, '0%', 'the bar entry is built through panelEntry with the raw map');
  assert.equal(model.isAlarming(shaped), false);
  assert.equal(model.headline(model.panelEntry(zai, {}, {}, {})).text, '100%');
  const agy = model.parseReport(JSON.stringify({entries: [{id: 'antigravity', sections: [
    {type: 'text', label: 'Session', value: ''},
    {type: 'metric', label: 'Gemini', percent: 10},
    {type: 'metric', label: 'Claude & GPT OSS', percent: 20},
    {type: 'text', label: 'Weekly', value: ''},
    {type: 'metric', label: 'Gemini', percent: 90},
    {type: 'metric', label: 'Claude & GPT OSS', percent: 30}
  ]}]})).entries[0];
  const on = {gemini: true, third_party: true};
  const hiddenWeeklyGemini = model.panelEntry(agy, {antigravity: ['Weekly / Gemini']}, {}, on);
  assert.equal(Array.from(hiddenWeeklyGemini.sections).filter(row => row.type === 'metric').length, 3,
    'a hand-edited hiddenMetrics still applies to Antigravity beside its pool switches');
  const choices = model.metricChoices(agy, {});
  assert.deepEqual(Array.from(choices, row => row.key + '|' + row.group), ['window:session|', 'window:weekly|']);
}

assert.equal(model.cursorPoolOf({type: 'metric', label: 'Cursor Models'}), 'models');
assert.equal(model.cursorPoolOf({type: 'metric', label: 'Other Models'}), 'other');
assert.equal(model.cursorPoolOf({type: 'metric', label: 'On-Demand'}), 'demand');
assert.equal(model.cursorPoolOf({type: 'metric', label: 'Team credit'}), 'credits');
assert.equal(model.cursorPoolOf({type: 'text', label: 'On-Demand'}), 'demand', 'the On-Demand text row has its pool too');
assert.equal(model.cursorPoolOf({type: 'text', label: 'Redefinições'}), '');
assert.equal(model.cursorPoolOf(null), '');

{
  const claude = model.parseReport(JSON.stringify({entries: [
    {id: 'anthropic', short_name: 'cld', icon: 'C', sections: [{type: 'metric', label: 'Session', percent: 10}]},
    {id: 'openai', short_name: 'gpt', icon: 'G', sections: [{type: 'metric', label: 'Session', percent: 20}]}
  ]})).entries;
  const chips = (entries, selected, all, brandIcons) =>
    model.barChips(entries, selected, all, true, false, false, false, false, 'auto', 'used', brandIcons);
  assert.equal(chips(claude, claude[0], false)[0].brand, 'claude.svg');
  assert.equal(chips(claude, claude[0], false, true)[0].brand, 'claude.svg');
  assert.equal(chips(claude, claude[0], false, undefined)[0].brand, 'claude.svg', 'an older caller keeps the marks');
  const generic = chips(claude, claude[0], false, false);
  assert.equal(generic.length, 1);
  assert.equal(generic[0].brand, '');
  assert.equal(generic[0].icon, '󰚩');
  assert.equal(generic[0].label, '10%');
  assert.equal(generic[0].labelOnly, false);
  const many = chips(claude, claude[0], true, false);
  assert.deepEqual(Array.from(many.map(chip => chip.brand)), ['', '']);
  assert.deepEqual(Array.from(many.map(chip => chip.icon)), ['', ''], 'no glyph box: a 3-letter tag would overflow it');
  assert.deepEqual(Array.from(many.map(chip => chip.labelOnly)), [true, true]);
  assert.deepEqual(Array.from(many.map(chip => chip.label)), ['cld 10%', 'gpt 20%'], 'the tag leads the label, one space from the value');
  const tagged = (all, brandIcons) =>
    model.barChips(claude, claude[0], all, true, true, false, false, false, 'auto', 'used', brandIcons);
  assert.deepEqual(Array.from(tagged(true, false).map(chip => chip.label)), ['cld 10%', 'gpt 20%'],
    'the tag is already leading the label, so it is not repeated');
  const lone = tagged(false, false)[0];
  assert.equal(lone.icon, '󰚩');
  assert.equal(lone.label, 'cld 10%', 'a lone chip keeps the tag in its label');
  assert.deepEqual(Array.from(tagged(true, true).map(chip => chip.label)), ['cld 10%', 'gpt 20%']);
  assert.deepEqual(Array.from(tagged(true, true).map(chip => chip.labelOnly)), [false, false], 'the marks keep their own box');
  const brandless = model.parseReport(JSON.stringify({entries: [
    {id: 'commandcode', short_name: 'cmd', icon: 'X', sections: [{type: 'metric', label: 'Session', percent: 5}]}
  ]})).entries;
  const bare = (showProvider) =>
    model.barChips(brandless, brandless[0], false, true, showProvider, false, false, false, 'auto', 'used', true)[0];
  assert.equal(bare(false).label, 'cmd 5%', 'a provider with no mark leads with its tag, spaced');
  assert.equal(bare(false).icon, '');
  assert.equal(bare(true).label, 'cmd 5%', 'and does not repeat it when the provider name is on');
}

{
  const panel = fs.readFileSync(new URL('./Panel.qml', import.meta.url), 'utf8');
  const settingsForm = fs.readFileSync(new URL('./SettingsView.qml', import.meta.url), 'utf8');
  assert.match(panel, /Model\.normalizeShowAs\(setting\("showAs",\s*"used"\)\)/);
  assert.match(panel, /Model\.normalizeHiddenMetrics\(setting\("hiddenMetrics",\s*\{\}\)\)/);
  assert.match(panel, /Model\.panelEntry\(\s*item,\s*hiddenMetrics,\s*cursorPoolFlags\(\)/);
  assert.doesNotMatch(panel, /hiddenKeysFor\(hiddenMetrics/, 'panelEntry takes the raw map and resolves the entry id itself');
  assert.match(panel, /persistWidgetSettings\(\{\s*showAs:\s*next\s*\}\)/);
  assert.match(panel, /if \(!Model\.canToggleMetric\(target,\s*hiddenMetrics,\s*key\)\) return/);
  assert.match(panel, /hiddenMetrics:\s*Model\.toggleMetricChoice\(hiddenMetrics,\s*target,\s*key\)/);
  assert.match(panel, /Model\.metricChoices\(item,\s*hiddenMetrics\)/);
  assert.match(panel, /onMetricToggleRequested:\s*function\(entryId,\s*key\)\s*\{\s*root\.setMetricShown\(entryId,\s*key\)\s*\}/);
  assert.doesNotMatch(panel, /metricEye|hideable|hasHideableMetrics|metric\.hidden_hint/, 'rows carry no switch; the choice lives in Settings');
  assert.match(panel, /onShowAsRequested:\s*function\(value\)\s*\{\s*root\.setShowAs\(value\)\s*\}/);
  assert.match(panel, /function shownAlarming\(\) \{\s*return entryIsAlarming\(shapedEntry\)/, 'the alert reads the entry without hidden metrics');
  assert.match(panel, /entryIsAlarming\(shapedEntries\[i\]\)/);
  assert.match(panel, /readonly property var valueView:\s*Model\.metricValueView\(row,\s*root\.showAs\)/);
  assert.match(settingsForm, /property string openSection:\s*"display"/);
  assert.match(settingsForm, /function toggleSection\(id\) \{\s*openSection = openSection === id \? "" : id\s*\}/);
  assert.equal((settingsForm.match(/^    Disclosure \{/gm) || []).length, 8);
  for (const id of ['display', 'language', 'barWindow', 'showAs', 'metrics', 'primary', 'providers', 'credentials']) {
    assert.match(settingsForm, new RegExp(`onToggled: root\\.toggleSection\\("${id}"\\)`), `${id} folds`);
    assert.match(settingsForm, new RegExp(`visible: root\\.openSection === "${id}"`), `${id} body`);
  }
  assert.doesNotMatch(settingsForm, /providersOpen|credentialsOpen/);
  assert.doesNotMatch(settingsForm, /^    PanelSectionHeader \{\s*\n\s*text: root\.tr\("section\.(display|language|bar_window|show_as|primary|providers|credentials)"\)/m);
  assert.match(panel, /labelOnly:\s*chip\.labelOnly/);
  assert.match(panel, /if \(showProvider \|\| chip\.labelOnly === true\)/);
  assert.match(barWidgetSource, /visible:\s*chipHit\.chip\.labelOnly !== true/);
  assert.match(settingsForm, /signal showAsRequested\(string value\)/);
  assert.match(settingsForm, /modelData\.labelKey !== ""\s*\? root\.tr\(modelData\.labelKey\)/, 'window rows are worded in the UI language');
  assert.match(settingsForm, /signal metricToggleRequested\(string entryId, string key\)/);
  assert.match(settingsForm, /enabled: !root\.saving && modelData\.canToggle/);
  assert.match(settingsForm, /opacity: modelData\.canToggle \? 1 : 0\.45/, 'a locked switch looks locked');
  assert.match(settingsForm, /onChanged:\s*function\(value\)\s*\{\s*root\.showAsRequested\(value\)\s*\}/);
  assert.match(panel, /Model\.booleanSetting\(setting\("brandIcons",\s*true\),\s*true\)/);
  assert.match(panel, /barWindow,\s*showAs,\s*brandIcons\)/);
  assert.match(panel, /persistWidgetSettings\(\{\s*brandIcons:\s*next\s*\}\)/);
  assert.match(panel, /brand: root\.settingsOpen \|\| !root\.brandIcons \? "" : Model\.brandIconFile\(root\.entry\)/);
  assert.match(settingsForm, /signal brandIconsRequested\(bool enabled\)/);
  const displayKeys = ['showValue', 'brandIcons', 'showProvider', 'colorCodeUsage', 'showAll'];
  assert.deepEqual(displayKeys.map(key => manifest.barWidget.defaults[key]), [true, true, false, false, false]);
  assert.deepEqual(manifest.barWidget.schema.filter(row => displayKeys.includes(row.key)).map(row => row.key), displayKeys);
  assert.deepEqual(manifest.barWidget.schema.filter(row => displayKeys.includes(row.key)).map(row => row.defaultValue), [true, true, false, false, false]);
  assert.deepEqual(
    Array.from(settingsForm.matchAll(/label: root\.tr\("toggle\.(\w+)"\)/g), match => match[1]),
    ['show_value', 'brand_icons', 'show_provider', 'color_code', 'show_all']);
  assert.equal(manifest.barWidget.defaults.brandIcons, true);
  assert.equal(manifest.barWidget.schema.find(row => row.key === 'brandIcons').type, 'boolean');
  assert.equal(manifest.barWidget.defaults.showAs, 'used');
  const showAsSchema = manifest.barWidget.schema.find(row => row.key === 'showAs');
  assert.equal(showAsSchema.type, 'enum');
  assert.deepEqual(showAsSchema.options, ['used', 'left']);
  assert.equal(showAsSchema.defaultValue, 'used');
}

console.log('Omarchy model tests passed');
