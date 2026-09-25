import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  accountSwitchFor,
  formatDuration,
  nextUpdateLabel,
  parseHostPayload,
  projectCards,
  resetLabel,
  friendlyError,
  explainError,
  displayPlan,
  leftLabel,
  emptyLayout,
  memoryStorage,
  loadLayout,
  saveLayout,
  syncLayout,
  applyCardLayout,
  orderedCards,
  moveCardBefore,
  nudgeCard,
  visibleRowsFor,
  cardHasExtras,
  rowKey,
  mergeRowPrefs,
  defaultRowPrefs,
  quotaLabel,
  quotaAlternate,
  headlineLabel,
  headlineAlternate,
  lacksCredentials,
  seedLayout,
  hintPending,
  absorbPayload,
  setRowEnabled,
  moveRowToList,
  LAYOUT_KEY,
  normalizeLayout,
  meterColor,
  resetText,
  resetAlternate,
  formatResetExact,
  formatResetCreditDate,
  resetCreditDetails,
  condensedTextRowIndexes,
  providerIconId,
  initialsGlyph,
  metricCount,
  sendCommand,
  resolvedTheme,
  emptyPayload,
  pace,
  paceText,
  paceTickPercent,
  paceVisible,
  usageGoal,
  prettyMetricLabel,
  shortcutFromKeyEvent,
  defaultStars,
  toggleStar,
  metricRowKey,
  paceWarmupMs,
  paceWarmupText,
  paceWarmupHint,
  isStarred,
  seedStars,
  stripCommand,
  MAX_STARS_PER_PROVIDER,
  expirySeverity,
  providerLinks,
  isHttpUrl,
  formatAgo,
  updateStatusLabel,
  updateBannerPending,
  updateModeLabel,
  updateAction,
  updateMessage,
} from './src/model.js';
import { measurePanelHeight } from './src/panel-size.js';

// The native window can retain a tall viewport while WebView2 is hidden. Its
// stretched scroll region must not become the next requested panel height.
const intrinsicContent = { offsetHeight: 420, scrollHeight: 510 };
const stretchedScroller = { dataset: { scroll: '' }, offsetHeight: 900 };
const footerChrome = { dataset: {}, offsetHeight: 44 };
assert.equal(measurePanelHeight({
  children: [stretchedScroller, footerChrome],
  querySelector: () => intrinsicContent,
}), 554);

const report = {
  version: '1.10.0',
  generated_at: 1_000,
  next_refresh_at: 61_000,
  startup_enabled: true,
  host_error: null,
  menu_bar_show_all: false,
  menu_bar_hide_value: true,
  menu_bar_window: 'weekly',
  menu_bar_chart: true,
  primary: 'anthropic',
  entries: [
    {
      id: 'anthropic',
      display_name: 'Claude',
      short_name: 'cld',
      plan: 'Team 5x',
      status: 'ready',
      error: null,
      stale: false,
      sections: [
        {
          type: 'metric',
          label: 'Weekly',
          percent: 19,
          value: '19%',
          detail: 'Resets in 1d 16h',
          severity: 'low',
          reset_at: '2026-09-05T00:00:00Z',
        },
        {
          type: 'text',
          label: 'Balance',
          value: '$12.50',
        },
        { type: 'spacer' },
      ],
    },
  ],
};

const payload = parseHostPayload(report);
assert.equal(payload.version, '1.10.0');
assert.equal(payload.startupEnabled, true);
assert.equal(payload.menuBarShowAll, false);
assert.equal(payload.menuBarHideValue, true);
assert.equal(payload.menuBarWindow, 'weekly');
assert.equal(payload.menuBarChart, true);
assert.equal(payload.menuBarProvider, 'highest');
assert.equal(payload.notificationsEnabled, true);
assert.equal(payload.notificationsThreshold, 97);
assert.equal(parseHostPayload({ notifications_enabled: false, notifications_threshold: 85 }).notificationsEnabled, false);
assert.equal(parseHostPayload({ notifications_threshold: 85 }).notificationsThreshold, 85);
assert.equal(parseHostPayload({ notifications_threshold: 101 }).notificationsThreshold, 97);
assert.equal(parseHostPayload({ menu_bar_provider: 'openai@work' }).menuBarProvider, 'openai@work');
assert.equal(payload.entries.length, 1);
assert.equal(payload.entries[0].displayName, 'Claude');
assert.equal(payload.entries[0].sections.length, 2); // spacer dropped

assert.equal(formatDuration(0), 'now');
assert.equal(formatDuration(90_000), '1m');
assert.equal(formatDuration(3_600_000 + 120_000), '1h 2m');
assert.equal(formatDuration(2 * 86_400_000 + 3_600_000), '2d 1h');

const cards = projectCards(payload, Date.parse('2026-09-04T00:00:00Z'));
assert.equal(cards.length, 1);
assert.equal(cards[0].title, 'Claude');
assert.equal(cards[0].plan, 'Team 5x');
assert.equal(cards[0].rows[0].kind, 'metric');
assert.equal(cards[0].rows[0].leftPercent, 81);
assert.equal(cards[0].rows[0].usedPercent, 19);
assert.equal(cards[0].rows[0].reset, 'Resets in 1d 0h');
assert.equal(cards[0].rows[1].kind, 'text');
assert.equal(cards[0].rows[1].value, '$12.50');

assert.equal(
  resetLabel({ type: 'metric', resetAt: '', detail: 'Resets in 3h' }, 0),
  'Resets in 3h',
);

assert.equal(nextUpdateLabel(payload, 1_000), 'Next update in 1m');
assert.equal(nextUpdateLabel(payload, 61_000), 'Updating…');

const bad = parseHostPayload('{');
assert.equal(bad.hostError.includes('valid JSON'), true);
assert.equal(bad.entries.length, 0);

const stripped = parseHostPayload({
  version: '1',
  entries: [{
    id: 'x',
    display_name: 'Hi\u0007 there',
    sections: [{ type: 'metric', label: 'S', percent: 150, severity: 'nope' }],
  }],
});
assert.equal(stripped.entries[0].displayName.includes('\u0007'), false);
assert.equal(stripped.entries[0].sections[0].percent, 100);
assert.equal(stripped.entries[0].sections[0].severity, 'low');

const failed = parseHostPayload({
  host_error: 'no vendors enabled',
  entries: [{ id: 'openai', display_name: 'Codex', status: 'error', error: 'not signed in', sections: [] }],
});
assert.equal(failed.hostError, 'no vendors enabled');
assert.equal(failed.entries[0].status, 'error');
assert.equal(failed.entries[0].error, 'not signed in');

const errored = parseHostPayload({
  entries: [{
    id: 'openai',
    display_name: 'Codex',
    status: 'error',
    error: 'not signed in',
      sign_in: 'Run `codex login` in a terminal, then Refresh.',
    stale: true,
    sections: [],
  }],
});
const errorCards = projectCards(errored, 0);
assert.equal(errorCards[0].errorTitle, 'Sign-in expired');
assert.equal(errorCards[0].errorHint, 'Run `codex login` in a terminal, then Refresh.');
assert.equal(errorCards[0].error, 'Sign-in expired. Run `codex login` in a terminal, then Refresh.');
assert.equal(errorCards[0].errorDetail, 'not signed in');
assert.equal(errorCards[0].stale, true);
assert.equal(errorCards[0].rows.length, 0);

const claudeErrored = parseHostPayload({
  entries: [{
    id: 'anthropic',
    display_name: 'Claude',
    plan: 'Claude Max 5x',
    status: 'error',
    error: 'HTTP 401: authentication rejected — credentials may be missing, expired, or invalid',
    sign_in: 'Run `claude` in a terminal, then Refresh.',
    sections: [],
  }],
});
const claudeCards = projectCards(claudeErrored, 0);
assert.equal(claudeCards[0].title, 'Claude');
assert.equal(claudeCards[0].plan, 'Claude Max 5x');
assert.equal(claudeCards[0].errorTitle, 'Sign-in expired');
assert.equal(claudeCards[0].errorHint, 'Run `claude` in a terminal, then Refresh.');
assert.equal(claudeCards[0].rows.length, 0);

const none = parseHostPayload({ version: '1', entries: [] });
assert.equal(none.entries.length, 0);
assert.equal(projectCards(none, 0).length, 0);
assert.equal(nextUpdateLabel({ nextRefreshAt: 0 }, 1), 'Updating…');

assert.equal(
  friendlyError('Zai: no API key. Either set an API key in a valid environment variable or set `api_key` under [zai] in C:\\Users\\dj4lm\\AppData\\Roaming\\ai-usagebar\\config\\config.toml.'),
  'No API key. Open TUI → Settings to add one.',
);
assert.equal(
  friendlyError('HTTP 429: Rate limited. Please try again later.'),
  'Too many requests. Try Refresh in a minute.',
);
assert.equal(
  explainError('network transport error: connection refused', 'openai').title,
  "Can't reach the server",
);
assert.equal(
  explainError('HTTP 503: provider unavailable', 'zai').title,
  'Provider is unavailable',
);
assert.ok(!friendlyError('Zai: no API key in C:\\Users\\dj4lm\\AppData\\Roaming\\ai-usagebar\\config\\config.toml.').includes('AppData'));

assert.equal(displayPlan('Claude', 'Claude Max 5x'), 'Max 5x');
assert.equal(displayPlan('Claude', 'Team 5x'), 'Team 5x');
assert.equal(displayPlan('Codex', 'Plus'), 'Plus');
assert.equal(leftLabel(81), '81% left');
assert.equal(leftLabel(0), 'Limit reached');
assert.equal(quotaLabel({ kind: 'metric', leftPercent: 81, usedPercent: 19 }, 'left'), '81% left');
assert.equal(quotaLabel({ kind: 'metric', leftPercent: 81, usedPercent: 19 }, 'used'), '19% used');
assert.equal(quotaLabel({ kind: 'metric', leftPercent: 0, usedPercent: 100 }, 'used'), 'Limit reached');

const exhausted = parseHostPayload({
  entries: [{
    id: 'cursor',
    display_name: 'Cursor',
    plan: 'Ultra',
    sections: [
      { type: 'metric', label: 'Total', percent: 27, severity: 'low' },
      { type: 'metric', label: 'Auto', percent: 2, severity: 'low' },
      { type: 'metric', label: 'API', percent: 100, severity: 'critical' },
      { type: 'text', label: 'Extra Usage', value: '$364.04 spent' },
    ],
  }],
});
const cursorCard = projectCards(exhausted, 0)[0];
assert.equal(cursorCard.rows[2].leftPercent, 0);
assert.equal(leftLabel(cursorCard.rows[2].leftPercent), 'Limit reached');
assert.equal(cardHasExtras(cursorCard, false), true);
assert.equal(visibleRowsFor(cursorCard, { collapsed: true }).map((r) => r.kind).join(','), 'metric,metric');
assert.equal(visibleRowsFor(cursorCard, { collapsed: false, hideExtras: true }).length, 3);
assert.equal(visibleRowsFor(cursorCard, { collapsed: false, hideExtras: false }).length, 4);
assert.equal(rowKey(cursorCard.rows[0]), 'metric:Total');

// A "Resets" text section repeats the countdown every meter already shows.
const withResets = parseHostPayload({
  entries: [
    {
      id: 'cursor',
      display_name: 'Cursor',
      sections: [
        { type: 'metric', label: 'Cursor Models', percent: 32, severity: 'low', reset_at: '2026-09-09T00:00:00Z' },
        { type: 'text', label: 'Resets', value: '4d 14h' },
      ],
    },
    {
      id: 'zai',
      display_name: 'Z.AI',
      sections: [
        { type: 'metric', label: 'Balance', percent: 10, severity: 'low' },
        { type: 'text', label: 'Resets', value: '12d 2h' },
      ],
    },
  ],
});
const [resetsFolded, resetsKept] = projectCards(withResets, 0);
assert.equal(resetsFolded.rows.map((r) => r.kind).join(','), 'metric');
assert.equal(resetsKept.rows.map((r) => r.label).join(','), 'Balance,Resets');

// Banked resets stay structured through normalization so the dashboard can
// keep the compact count in the card and put every expiry in its tooltip.
const bankedResets = parseHostPayload({
  entries: [{
    id: 'openai',
    display_name: 'Codex',
    reset_credits: {
      available: 3,
      credits: [
        { title: 'Full reset', expires_at: '2026-10-05T04:18:00Z' },
        { title: 'Full reset', expires_at: '2026-09-20T23:58:00Z' },
      ],
    },
    sections: [{ type: 'block', label: 'Reset credits', body: ['legacy text'] }],
  }],
});
assert.equal(bankedResets.entries[0].resetCredits.available, 3);
assert.equal(bankedResets.entries[0].resetCredits.credits[0].expiresAt, '2026-10-05T04:18:00Z');
const resetRow = projectCards(bankedResets, Date.parse('2026-09-15T10:00:00Z'))[0].rows[0];
assert.equal(resetRow.kind, 'resetCredits');
assert.equal(resetRow.label, 'Rate Limit Resets');
assert.equal(resetRow.available, 3);

const resetDetails = resetCreditDetails(resetRow, Date.parse('2026-09-15T10:00:00Z'), {
  locale: 'en-US',
  timeZone: 'UTC',
  timeFormat: '24',
});
assert.deepEqual(resetDetails.items.map((item) => item.date), [
  'Sep 20 at 23:58',
  'Oct 5 at 04:18',
  'Date unavailable',
]);
assert.deepEqual(resetDetails.items.map((item) => item.remaining), ['5d 13h', '19d 18h', '—']);
// Dots are colored by how soon each credit expires, never by position; an
// unknown date gets the neutral dot.
assert.deepEqual(resetDetails.items.map((item) => item.severity), ['yellow', 'blue', '']);

assert.equal(resetDetails.hidden, 0);
assert.equal(
  formatResetCreditDate('2026-09-20T23:58:00Z', { locale: 'en-US', timeZone: 'UTC', timeFormat: '12' }),
  'Sep 20 at 11:58 PM',
);

// Older hosts do not send `reset_credits`; their existing text block remains visible.
const legacyResetCard = projectCards(parseHostPayload({
  entries: [{
    id: 'openai',
    display_name: 'Codex',
    sections: [{ type: 'block', label: 'Reset credits', body: ['2 resets available'] }],
  }],
}), 0)[0];
assert.equal(legacyResetCard.rows[0].kind, 'block');

const extraOnAlways = moveRowToList(
  defaultRowPrefs(cursorCard.rows),
  'text:Extra Usage',
  'always',
  null,
);
assert.deepEqual(extraOnAlways.always.slice(-1), ['text:Extra Usage']);
assert.equal(visibleRowsFor(cursorCard, { collapsed: true, prefs: extraOnAlways }).length, 3);
const cursorPrefs = defaultRowPrefs(cursorCard.rows);
const disabledApi = setRowEnabled(cursorPrefs, 'metric:API', false);
assert.equal(disabledApi.off['metric:API'], true);
assert.deepEqual(disabledApi.always, cursorPrefs.always);
assert.deepEqual(disabledApi.demand, cursorPrefs.demand);
assert.equal(visibleRowsFor(cursorCard, { collapsed: false, prefs: disabledApi }).length, 3);
const reenabledApi = setRowEnabled(disabledApi, 'metric:API', true);
assert.equal(reenabledApi.off['metric:API'], undefined);
assert.deepEqual(reenabledApi.always, cursorPrefs.always);
assert.deepEqual(reenabledApi.demand, cursorPrefs.demand);
const mergedStored = mergeRowPrefs(cursorCard.rows, {
  always: ['metric:API'],
  demand: ['metric:Total'],
  off: { 'text:Extra Usage': true },
});
assert.ok(mergedStored.always.indexOf('metric:API') >= 0);
assert.ok(mergedStored.demand.indexOf('metric:Total') >= 0);
assert.equal(mergedStored.off['text:Extra Usage'], true);

const three = [
  { id: 'anthropic', title: 'Claude' },
  { id: 'openai', title: 'Codex' },
  { id: 'cursor', title: 'Cursor' },
];
assert.deepEqual(
  applyCardLayout(three, { cardOrder: ['cursor', 'anthropic'], hidden: { openai: true }, collapsed: {}, hideExtras: false }).map((c) => c.id),
  ['cursor', 'anthropic'],
);
assert.deepEqual(moveCardBefore(['a', 'b', 'c'], 'c', 'a'), ['c', 'a', 'b']);
assert.deepEqual(moveCardBefore(['a', 'b', 'c'], 'a', null), ['b', 'c', 'a']);
assert.deepEqual(nudgeCard(['a', 'b', 'c'], 'b', -1), ['b', 'a', 'c']);
assert.deepEqual(orderedCards(three, { cardOrder: ['cursor'] }).map((c) => c.id), ['cursor', 'anthropic', 'openai']);

const store = memoryStorage();
saveLayout(store, { cardOrder: ['cursor', 'openai'], hidden: { openai: true }, collapsed: { cursor: true }, hideExtras: true });
const loaded = loadLayout(store);
assert.equal(loaded.hideExtras, true);
assert.equal(loaded.hidden.openai, true);
assert.equal(loaded.collapsed.cursor, true);
assert.deepEqual(loaded.cardOrder, ['cursor', 'openai']);
const synced = syncLayout(loaded, ['anthropic', 'openai', 'cursor']);
assert.deepEqual(synced.cardOrder, ['cursor', 'openai', 'anthropic']);
assert.ok(store.getItem(LAYOUT_KEY).includes('cursor'));
assert.deepEqual(emptyLayout().cardOrder, []);

// Language must survive storage normalization and every host payload refresh.
const languageStore = memoryStorage();
saveLayout(languageStore, { ...emptyLayout(), language: 'pt-BR' });
assert.equal(loadLayout(languageStore).language, 'pt-BR');
assert.equal(syncLayout(loadLayout(languageStore), ['anthropic']).language, 'pt-BR');
assert.equal(normalizeLayout({ language: 'invalid' }).language, 'en');
assert.equal(nextUpdateLabel({ nextRefreshAt: 120_000 }, 60_000, 'pt-BR'), 'Próxima atualização em 1m');
assert.equal(resetText({ resetAt: '2026-09-24T12:00:00Z' }, 'countdown', Date.parse('2026-09-24T11:00:00Z'), { locale: 'pt-BR' }), 'Redefine em 1h 0m');
assert.match(formatResetExact(Date.parse('2026-09-24T12:00:00Z'), Date.parse('2026-09-24T11:00:00Z'), { locale: 'pt-BR', timeZone: 'UTC', timeFormat: '24' }), /^hoje às 12:00$/);
assert.equal(updateStatusLabel({ update: null, updateCheckedAt: 0 }, 0, 'pt-BR'), 'Ainda não verificado');

// --- resetTimes layout field ------------------------------------

{
  const empty = emptyLayout();
  assert.equal(empty.resetTimes, 'countdown');
}

{
  const exact = normalizeLayout({ resetTimes: 'exact' });
  const junk = normalizeLayout({ resetTimes: 'never' });
  const missing = normalizeLayout({});
  assert.equal(exact.resetTimes, 'exact');
  assert.equal(junk.resetTimes, 'countdown');
  assert.equal(missing.resetTimes, 'countdown');
}

{
  const store = memoryStorage();
  saveLayout(store, { cardOrder: ['cursor'], resetTimes: 'exact' });
  const reloaded = loadLayout(store);
  const resynced = syncLayout(reloaded, ['anthropic', 'cursor']);
  assert.equal(reloaded.resetTimes, 'exact');
  assert.equal(resynced.resetTimes, 'exact');
  assert.deepEqual(resynced.cardOrder, ['cursor', 'anthropic']);
}

{
  const cleaned = syncLayout({ cardOrder: [], resetTimes: 'maybe' }, ['anthropic']);
  assert.equal(cleaned.resetTimes, 'countdown');
}

// --- meterColor --------------------------------------------------------------

assert.equal(meterColor('low'), 'blue');
assert.equal(meterColor('mid'), 'yellow');
assert.equal(meterColor('high'), 'yellow');
assert.equal(meterColor('critical'), 'red');
assert.equal(meterColor('nope'), 'blue');
assert.equal(meterColor(undefined), 'blue');
assert.equal(meterColor('low', { state: 'ahead', sparePercent: 40 }), 'blue');
assert.equal(meterColor('low', { state: 'onTrack', sparePercent: 2 }), 'yellow');
assert.equal(meterColor('low', { state: 'onTrack', sparePercent: 0 }), 'red');
assert.equal(meterColor('low', { state: 'behind', sparePercent: -12 }), 'red');
assert.equal(meterColor('low', null, true), 'red');

// --- resetText / resetAlternate / formatResetExact ---------------------------

const resetNow = Date.parse('2026-09-04T12:00:00Z');
const utc = { timeZone: 'UTC' };
const sameDayRow = { kind: 'metric', resetAt: '2026-09-04T18:38:00Z', reset: 'Resets in 6h 38m' };
const nextDayRow = { kind: 'metric', resetAt: '2026-09-05T18:38:00Z', reset: 'Resets in 1d 6h' };
const laterRow = { kind: 'metric', resetAt: '2026-09-12T18:38:00Z', reset: 'Resets in 8d 6h' };
const noStampRow = { kind: 'metric', resetAt: '', reset: 'Resets in 1d 16h' };
const badStampRow = { kind: 'metric', resetAt: 'not-a-date', reset: 'Resets in 2h' };

// countdown mode uses the live clock, not the row's cached text
assert.equal(resetText(sameDayRow, 'countdown', resetNow, utc), 'Resets in 6h 38m');
assert.equal(resetText(nextDayRow, 'countdown', resetNow, utc), 'Resets in 1d 6h');

// exact mode: today / tomorrow / other day
assert.equal(resetText(sameDayRow, 'exact', resetNow, utc), 'Resets today at 6:38 PM');
assert.equal(resetText(nextDayRow, 'exact', resetNow, utc), 'Resets tomorrow at 6:38 PM');
assert.equal(resetText(laterRow, 'exact', resetNow, utc), 'Resets Sep 12 at 6:38 PM');

// no usable timestamp falls back to the row text in either mode
assert.equal(resetText(noStampRow, 'countdown', resetNow, utc), 'Resets in 1d 16h');
assert.equal(resetText(noStampRow, 'exact', resetNow, utc), 'Resets in 1d 16h');
assert.equal(resetText(badStampRow, 'exact', resetNow, utc), 'Resets in 2h');
assert.equal(resetText({ kind: 'metric', resetAt: '', reset: '' }, 'exact', resetNow, utc), '');
assert.equal(resetText(null, 'exact', resetNow, utc), '');

// formatResetExact on its own, and the calendar-day comparison honors the zone
assert.equal(formatResetExact(Date.parse('2026-09-04T18:38:00Z'), resetNow, utc), 'today at 6:38 PM');
assert.equal(formatResetExact(Date.parse('2026-09-05T00:05:00Z'), resetNow, utc), 'tomorrow at 12:05 AM');
assert.equal(
  formatResetExact(Date.parse('2026-09-05T00:05:00Z'), resetNow, { timeZone: 'America/Sao_Paulo' }),
  'today at 9:05 PM',
);

// resetAlternate flips the mode; nothing to flip without a timestamp
assert.equal(resetAlternate(sameDayRow, 'countdown', resetNow, utc), 'Resets today at 6:38 PM');
assert.equal(resetAlternate(sameDayRow, 'exact', resetNow, utc), 'Resets in 6h 38m');
assert.equal(resetAlternate(noStampRow, 'countdown', resetNow, utc), '');
assert.equal(resetAlternate(badStampRow, 'exact', resetNow, utc), '');

// --- timeFormat: 12h / 24h override rides through resetText and resetAlternate --

{
  // ARRANGE: en-US would pick a 12-hour clock on its own
  const h24 = { locale: 'en-US', timeZone: 'UTC', timeFormat: '24' };
  const h12 = { locale: 'en-US', timeZone: 'UTC', timeFormat: '12' };
  const auto = { locale: 'en-US', timeZone: 'UTC', timeFormat: 'auto' };
  const at = Date.parse('2026-09-04T18:38:00Z');
  // ACT / ASSERT: the override wins in both directions, "auto" defers to the locale
  assert.equal(formatResetExact(at, resetNow, h24), 'today at 18:38');
  assert.equal(formatResetExact(at, resetNow, h12), 'today at 6:38 PM');
  assert.equal(formatResetExact(at, resetNow, auto), 'today at 6:38 PM');
  assert.equal(formatResetExact(Date.parse('2026-09-05T00:05:00Z'), resetNow, h24), 'tomorrow at 00:05');
  assert.equal(resetText(sameDayRow, 'exact', resetNow, h24), 'Resets today at 18:38');
  assert.equal(resetText(sameDayRow, 'exact', resetNow, h12), 'Resets today at 6:38 PM');
  assert.equal(resetAlternate(sameDayRow, 'countdown', resetNow, h24), 'Resets today at 18:38');
  assert.equal(resetAlternate(sameDayRow, 'exact', resetNow, h24), 'Resets in 6h 38m');
}

// --- layout: timeFormat / alwaysShowPace / usageGoal --------------------------

{
  // ARRANGE / ACT
  const empty = emptyLayout();
  const set = normalizeLayout({ timeFormat: '24', alwaysShowPace: true, usageGoal: true });
  const junk = normalizeLayout({ timeFormat: 'military', alwaysShowPace: 'yes', usageGoal: 'yes' });
  // ASSERT: defaults, valid values, and junk
  assert.equal(empty.timeFormat, 'auto');
  assert.equal(empty.alwaysShowPace, false);
  assert.equal(empty.usageGoal, false);
  assert.equal(set.timeFormat, '24');
  assert.equal(set.alwaysShowPace, true);
  assert.equal(set.usageGoal, true);
  assert.equal(junk.timeFormat, 'auto');
  assert.equal(junk.alwaysShowPace, false);
  assert.equal(junk.usageGoal, false);
  assert.equal(normalizeLayout({ timeFormat: '12' }).timeFormat, '12');

  // ASSERT: both survive storage and syncLayout
  const store = memoryStorage();
  saveLayout(store, { cardOrder: ['cursor'], timeFormat: '12', alwaysShowPace: true, usageGoal: true });
  const reloaded = loadLayout(store);
  assert.equal(reloaded.timeFormat, '12');
  assert.equal(reloaded.alwaysShowPace, true);
  assert.equal(reloaded.usageGoal, true);
  const synced = syncLayout(reloaded, ['cursor']);
  assert.equal(synced.timeFormat, '12');
  assert.equal(synced.alwaysShowPace, true);
  assert.equal(synced.usageGoal, true);
  const cleaned = syncLayout({ cardOrder: [], timeFormat: 'nope', alwaysShowPace: 1, usageGoal: 1 }, ['cursor']);
  assert.equal(cleaned.timeFormat, 'auto');
  assert.equal(cleaned.alwaysShowPace, false);
  assert.equal(cleaned.usageGoal, false);
}

// The goal follows wall-clock progress even when no usage has been reported.
{
  const end = Date.parse('2026-09-24T15:00:00Z');
  for (const seconds of [18_000, 604_800, 2_592_000]) {
    const row = { label: 'Session', resetAt: new Date(end).toISOString(), window: seconds, usedPercent: 0 };
    assert.deepEqual(usageGoal(row, end - seconds * 1000), { percent: 0, estimated: false });
    assert.deepEqual(usageGoal(row, end - seconds * 500), { percent: 50, estimated: false });
    assert.deepEqual(usageGoal(row, end), { percent: 100, estimated: false });
    assert.equal(usageGoal(row, end + 60_000), null);
  }
  const monthly = { label: 'Monthly', resetAt: '2026-03-31T12:00:00Z', window: 0 };
  assert.deepEqual(usageGoal(monthly, Date.parse('2026-02-28T12:00:00Z')), { percent: 0, estimated: true });
  assert.deepEqual(usageGoal(monthly, Date.parse('2026-03-31T12:00:00Z')), { percent: 100, estimated: true });
  assert.equal(usageGoal({ ...monthly, label: 'Weekly' }, end), null);
  assert.equal(usageGoal({ ...monthly, resetAt: 'bad' }, end), null);
}

// --- host payload: shortcut / updates / update / window_secs ------------------

{
  // ARRANGE: every new host key populated, with a GitHub release URL
  const full = parseHostPayload({
    version: '1.11.0',
    shortcut: 'Ctrl+Shift+U',
    shortcut_error: 'already taken',
    updates: 'auto',
    update: { state: 'downloading', version: 'v1.12.0', url: 'https://github.com/akitaonrails/ai-usagebar/releases/tag/v1.12.0', error: '' },
    update_checked_at: 1_700_000_000_000,
    entries: [{
      id: 'anthropic',
      display_name: 'Claude',
      sections: [
        { type: 'metric', label: 'Session', percent: 40, severity: 'low', reset_at: '2026-09-04T14:30:00Z', window_secs: 18000 },
        { type: 'metric', label: 'Weekly', percent: 10, severity: 'low', window_secs: -5 },
        { type: 'metric', label: 'Extra', percent: 5, severity: 'low', window_secs: 'lots' },
      ],
    }],
  });
  // ASSERT: the camelCase fields
  assert.equal(full.shortcut, 'Ctrl+Shift+U');
  assert.equal(full.shortcutError, 'already taken');
  assert.equal(full.updates, 'auto');
  assert.deepEqual(full.update, {
    error: '',
    installable: false,
    state: 'downloading',
    url: 'https://github.com/akitaonrails/ai-usagebar/releases/tag/v1.12.0',
    version: 'v1.12.0',
  });
  assert.equal(full.updateCheckedAt, 1_700_000_000_000);
  assert.equal(full.entries[0].sections[0].window, 18000);
  assert.equal(full.entries[0].sections[1].window, 0);
  assert.equal(full.entries[0].sections[2].window, 0);
  // the window rides onto the projected row
  const rows = projectCards(full, 0)[0].rows;
  assert.equal(rows[0].window, 18000);
  assert.equal(rows[1].window, 0);

  // ASSERT: defensive fallbacks
  const loose = parseHostPayload({
    updates: 'sometimes',
    update: { state: 'exploding', version: 'x'.repeat(50), url: 'https://evil.example/x', error: 'e'.repeat(400) },
    update_checked_at: 'yesterday',
  });
  assert.equal(loose.updates, 'notify');
  assert.equal(loose.update.state, 'available');
  assert.equal(loose.update.url, '');
  assert.equal(loose.update.version.length, 32);
  assert.equal(loose.update.error.length, 300);
  assert.equal(loose.updateCheckedAt, 0);
  assert.equal(parseHostPayload({ update: 'soon' }).update, null);
  assert.equal(parseHostPayload({ update: ['x'] }).update, null);
  assert.equal(parseHostPayload({ update_checked_at: Infinity }).updateCheckedAt, 0);
  assert.equal(parseHostPayload({ shortcut: 's'.repeat(80) }).shortcut.length, 64);

  // ASSERT: emptyPayload carries the defaults
  const empty = emptyPayload();
  assert.equal(empty.shortcut, '');
  assert.equal(empty.shortcutError, '');
  assert.equal(empty.updates, 'notify');
  assert.equal(empty.update, null);
  assert.equal(empty.updateCheckedAt, 0);
  assert.equal(payload.updates, 'notify');
  assert.equal(payload.update, null);

  // ASSERT: refresh_minutes keeps only the offered intervals, else the 5-minute default
  assert.equal(parseHostPayload({ refresh_minutes: 10 }).refreshMinutes, 10);
  assert.equal(parseHostPayload({ refresh_minutes: 1 }).refreshMinutes, 1);
  assert.equal(parseHostPayload({ refresh_minutes: 7 }).refreshMinutes, 5);
  assert.equal(parseHostPayload({ refresh_minutes: '10' }).refreshMinutes, 10);
  assert.equal(parseHostPayload({ refresh_minutes: 'often' }).refreshMinutes, 5);
  assert.equal(parseHostPayload({}).refreshMinutes, 5);
  assert.equal(empty.refreshMinutes, 5);
}

// --- pace / paceText / paceVisible --------------------------------------------

{
  // ARRANGE: a 5-hour window; `now` is the reference clock
  const window = 5 * 3600;
  const now = Date.parse('2026-09-04T12:00:00Z');
  const resetAfter = (ms) => new Date(now + ms).toISOString();
  const row = (usedPercent, elapsedMs, extra) => ({
    kind: 'metric',
    usedPercent,
    leftPercent: 100 - usedPercent,
    window,
    resetAt: resetAfter(window * 1000 - elapsedMs),
    ...extra,
  });

  // ACT / ASSERT: 40% spent at half the window projects 80% → ahead, 20% spare
  const ahead = pace(row(40, 2.5 * 3600_000), now);
  assert.equal(ahead.state, 'ahead');
  assert.equal(Math.round(ahead.projectedPercent), 80);
  assert.equal(ahead.sparePercent, 20);
  assert.equal(ahead.elapsedPercent, 50);
  assert.equal(ahead.runsOutMs, null);
  assert.equal(paceText(ahead, now), '~20% left at reset');
  assert.equal(paceVisible(ahead, { alwaysShowPace: false }), false);
  assert.equal(paceVisible(ahead, { alwaysShowPace: true }), true);
  assert.equal(paceVisible(ahead, emptyLayout()), false);

  // 45% at half the window is exactly the 90% boundary → still ahead
  assert.equal(pace(row(45, 2.5 * 3600_000), now).state, 'ahead');

  // 50% spent at half the window projects 100% → onTrack, nothing spare
  const close = pace(row(50, 2.5 * 3600_000), now);
  assert.equal(close.state, 'onTrack');
  assert.equal(close.sparePercent, 0);
  assert.equal(close.runsOutMs, null);
  assert.equal(paceText(close, now), '~0% spare');
  assert.equal(paceVisible(close, { alwaysShowPace: false }), true);

  // 48.5% at half the window projects 97% → onTrack with 3% spare
  const nearly = pace(row(48.5, 2.5 * 3600_000), now);
  assert.equal(nearly.state, 'onTrack');
  assert.equal(paceText(nearly, now), '~3% spare');
  // a negative spare never shows as "-N% spare"
  assert.equal(paceText({ state: 'onTrack', sparePercent: -1, projectedPercent: 101, runsOutMs: null, elapsedPercent: 50 }, now), '~0% spare');
  assert.equal(paceText({ state: 'ahead', sparePercent: 12, projectedPercent: 88, runsOutMs: null, elapsedPercent: 50 }, now), '~12% left at reset');

  // 80% spent after 2h projects 200% → behind, runs out at 2h30 (30m from now), before the reset
  const behind = pace(row(80, 2 * 3600_000), now);
  assert.equal(behind.state, 'behind');
  assert.equal(Math.round(behind.projectedPercent), 200);
  assert.equal(behind.sparePercent, -100);
  assert.equal(behind.elapsedPercent, 40);
  assert.ok(behind.runsOutMs > now);
  assert.ok(behind.runsOutMs < Date.parse(row(80, 2 * 3600_000).resetAt));
  assert.equal(Math.round((behind.runsOutMs - now) / 60_000), 30);
  assert.equal(paceText(behind, now), 'Limit in 30m');
  assert.equal(paceText(behind, now, { resetTimes: 'countdown' }), 'Limit in 30m');
  assert.equal(paceVisible(behind, { alwaysShowPace: false }), true);

  // exact reset times turn the countdown into a clock time (formatResetExact's wording)
  const exact = paceText(behind, now, { resetTimes: 'exact', timeZone: 'UTC' });
  assert.ok(exact.startsWith('Limit today at ') || exact.startsWith('Limit tomorrow at '), exact);
  assert.equal(exact, 'Limit ' + formatResetExact(behind.runsOutMs, now, { timeZone: 'UTC' }));
  // timeFormat flows through
  assert.equal(
    paceText(behind, now, { resetTimes: 'exact', timeZone: 'UTC', timeFormat: '24' }),
    'Limit today at 12:30',
  );
  assert.equal(
    paceText(behind, now, { resetTimes: 'exact', timeZone: 'UTC', timeFormat: '12' }),
    'Limit today at 12:30 PM',
  );

  // behind without a run-out instant (the limit is already spent) has no text: the flame alone
  const spent = pace(row(100, 2 * 3600_000), now);
  assert.equal(spent.state, 'behind');
  assert.equal(spent.runsOutMs, null);
  assert.equal(paceText(spent, now), '');
  assert.equal(paceText(spent, now, { resetTimes: 'exact', timeZone: 'UTC' }), '');

  // ASSERT: the tick follows the meter's reading — elapsed in Used mode, remaining in Left mode
  assert.equal(paceTickPercent(behind, 'used'), 40);
  assert.equal(paceTickPercent(behind, 'left'), 60);
  assert.equal(paceTickPercent(ahead, 'used'), 50);
  assert.equal(paceTickPercent(ahead, undefined), 50);
  assert.equal(paceTickPercent({ ...ahead, elapsedPercent: 250 }, 'used'), 100);
  assert.equal(paceTickPercent({ ...ahead, elapsedPercent: 250 }, 'left'), 0);
  assert.equal(paceTickPercent({ ...ahead, elapsedPercent: -5 }, 'used'), 0);
  assert.equal(paceTickPercent({ ...ahead, elapsedPercent: -5 }, 'left'), 100);
  assert.equal(paceTickPercent({ ...ahead, elapsedPercent: NaN }, 'used'), 0);
  assert.equal(paceTickPercent(null, 'used'), null);

  // ASSERT: no signal → null
  assert.equal(pace(row(50, 30_000), now), null); // 30s in: under the 1% / 60s floor
  assert.equal(pace(row(50, 179_000), now), null); // 1% of 5h is 3m
  assert.notEqual(pace(row(50, 180_000), now), null);
  assert.equal(pace(row(50, 2 * 3600_000, { window: 0 }), now), null);
  assert.equal(pace({ kind: 'metric', usedPercent: 50, leftPercent: 50, resetAt: resetAfter(3600_000) }, now), null);
  assert.equal(pace(row(50, 2 * 3600_000, { resetAt: 'not-a-date' }), now), null);
  assert.equal(pace(row(50, 2 * 3600_000, { resetAt: '' }), now), null);
  assert.equal(pace({ ...row(50, 2 * 3600_000), resetAt: resetAfter(-1) }, now), null); // already reset
  assert.equal(pace(row(0, 2 * 3600_000), now), null); // nothing spent: no rate to project
  assert.equal(pace(null, now), null);
  assert.equal(pace({ kind: 'text', label: 'Balance', value: '$1' }, now), null);
  assert.equal(paceText(null, now), '');
  assert.equal(paceVisible(null, { alwaysShowPace: true }), false);

  // ASSERT: a projected card row carries enough for pace() straight from the host payload
  const hosted = parseHostPayload({
    entries: [{ id: 'anthropic', display_name: 'Claude', sections: [
      { type: 'metric', label: 'Session', percent: 40, severity: 'low', reset_at: resetAfter(2.5 * 3600_000), window_secs: window },
    ] }],
  });
  assert.equal(pace(projectCards(hosted, now)[0].rows[0], now).state, 'ahead');
}

// --- shortcutFromKeyEvent ----------------------------------------------------

{
  const press = (code, mods, key) => ({
    key: key || '',
    code,
    ctrlKey: !!(mods && mods.ctrl),
    altKey: !!(mods && mods.alt),
    shiftKey: !!(mods && mods.shift),
    metaKey: !!(mods && mods.meta),
  });
  assert.equal(shortcutFromKeyEvent(press('KeyU', { ctrl: true, shift: true }, 'U')), 'Ctrl+Shift+U');
  assert.equal(shortcutFromKeyEvent(press('F5', { alt: true }, 'F5')), 'Alt+F5');
  assert.equal(shortcutFromKeyEvent(press('Space', { meta: true }, ' ')), 'Win+Space');
  assert.equal(shortcutFromKeyEvent(press('Digit1', { ctrl: true, alt: true }, '1')), 'Ctrl+Alt+1');
  assert.equal(shortcutFromKeyEvent(press('F24', { ctrl: true }, 'F24')), 'Ctrl+F24');
  assert.equal(shortcutFromKeyEvent(press('ArrowUp', { ctrl: true }, 'ArrowUp')), 'Ctrl+Up');
  assert.equal(shortcutFromKeyEvent(press('ArrowLeft', { ctrl: true, meta: true }, 'ArrowLeft')), 'Ctrl+Win+Left');
  assert.equal(shortcutFromKeyEvent(press('Backslash', { ctrl: true }, '\\')), 'Ctrl+\\');
  assert.equal(shortcutFromKeyEvent(press('Backquote', { alt: true }, '`')), 'Alt+`');
  assert.equal(shortcutFromKeyEvent(press('BracketLeft', { ctrl: true }, '[')), 'Ctrl+[');
  assert.equal(shortcutFromKeyEvent(press('Quote', { ctrl: true }, "'")), "Ctrl+'");
  assert.equal(shortcutFromKeyEvent(press('PageDown', { ctrl: true }, 'PageDown')), 'Ctrl+PageDown');
  // full modifier order: Ctrl, Alt, Shift, Win
  assert.equal(
    shortcutFromKeyEvent(press('KeyA', { ctrl: true, alt: true, shift: true, meta: true }, 'A')),
    'Ctrl+Alt+Shift+Win+A',
  );
  // no Ctrl/Alt/Win → null, even with Shift
  assert.equal(shortcutFromKeyEvent(press('KeyU', { shift: true }, 'U')), null);
  assert.equal(shortcutFromKeyEvent(press('KeyU', {}, 'u')), null);
  // modifier-only presses and Escape → null
  assert.equal(shortcutFromKeyEvent(press('ShiftLeft', { shift: true }, 'Shift')), null);
  assert.equal(shortcutFromKeyEvent(press('ControlLeft', { ctrl: true }, 'Control')), null);
  assert.equal(shortcutFromKeyEvent(press('AltRight', { alt: true }, 'Alt')), null);
  assert.equal(shortcutFromKeyEvent(press('MetaLeft', { meta: true }, 'Meta')), null);
  assert.equal(shortcutFromKeyEvent(press('Escape', { ctrl: true }, 'Escape')), null);
  // unknown codes → null
  assert.equal(shortcutFromKeyEvent(press('F25', { ctrl: true }, 'F25')), null);
  assert.equal(shortcutFromKeyEvent(press('NumpadAdd', { ctrl: true }, '+')), null);
  assert.equal(shortcutFromKeyEvent(press('CapsLock', { ctrl: true }, 'CapsLock')), null);
  assert.equal(shortcutFromKeyEvent(press('', { ctrl: true }, '')), null);
  assert.equal(shortcutFromKeyEvent(press('toString', { ctrl: true }, '')), null);
  assert.equal(shortcutFromKeyEvent(null), null);
}

// --- update status helpers -------------------------------------------------------

{
  const now = 1_700_000_000_000;
  assert.equal(formatAgo(0), 'just now');
  assert.equal(formatAgo(59_000), 'just now');
  assert.equal(formatAgo(5 * 60_000), '5m ago');
  assert.equal(formatAgo(2 * 3600_000 + 5 * 60_000), '2h ago');
  assert.equal(formatAgo(3 * 86_400_000 + 3600_000), '3d ago');
  assert.equal(formatAgo(-5), 'just now');

  const noUpdate = (checkedAt) => ({ ...emptyPayload(), updateCheckedAt: checkedAt });
  assert.equal(updateStatusLabel(noUpdate(0), now), 'Not checked yet');
  assert.equal(updateStatusLabel(noUpdate(now - 5 * 60_000), now), 'Up to date · checked 5m ago');
  assert.equal(updateStatusLabel(noUpdate(now - 10_000), now), 'Up to date · checked just now');

  const withUpdate = (state, extra) => ({
    ...emptyPayload(),
    updateCheckedAt: now,
    update: { error: '', state, url: '', version: '1.11.0', ...extra },
  });
  assert.equal(updateStatusLabel(withUpdate('available'), now), 'v1.11.0 available');
  assert.equal(updateStatusLabel(withUpdate('available', { version: 'v1.11.0' }), now), 'v1.11.0 available');
  assert.equal(updateStatusLabel(withUpdate('downloading'), now), 'Downloading v1.11.0…');
  assert.equal(updateStatusLabel(withUpdate('installing'), now), 'Installing…');
  assert.equal(updateStatusLabel(withUpdate('failed', { error: 'checksum mismatch' }), now), "Couldn't update: checksum mismatch");
  assert.equal(updateStatusLabel(withUpdate('failed'), now), "Couldn't update");
  assert.equal(updateStatusLabel(withUpdate('checking'), now), 'Checking…');
  assert.equal(parseHostPayload({ update: { state: 'checking', version: '' } }).update.state, 'checking');

  assert.equal(updateBannerPending(emptyPayload()), false);
  assert.equal(updateBannerPending(withUpdate('available')), true);
  assert.equal(updateBannerPending(withUpdate('failed')), true);
  assert.equal(updateBannerPending(withUpdate('checking')), false);
  assert.equal(updateBannerPending(withUpdate('failed', { version: '' })), false, 'a failed check is not an update');
  assert.equal(updateBannerPending(null), false);

  // One action table for the banner and the dialog.
  const repo = 'https://github.com/akitaonrails/ai-usagebar';
  const release = 'https://github.com/akitaonrails/ai-usagebar/releases/tag/v1.11.0';
  assert.equal(updateAction(null, repo).cmd, 'check-update');
  assert.equal(updateAction(withUpdate('checking').update, repo).busy, true);
  assert.equal(updateAction(withUpdate('downloading').update, repo).label, 'Downloading…');
  assert.deepEqual(updateAction(withUpdate('available', { installable: true }).update, repo), {
    busy: false,
    cmd: 'install-update',
    label: 'Install Update',
    url: '',
  });
  // Not installable here: never a dead Install, always the release page.
  assert.deepEqual(updateAction(withUpdate('available', { url: release }).update, repo), {
    busy: false,
    cmd: 'open-url',
    label: 'View Release',
    url: release,
  });
  assert.equal(updateAction(withUpdate('available').update, repo).url, repo + '/releases/latest');
  assert.equal(updateAction(withUpdate('failed').update, repo).cmd, 'install-update');
  assert.equal(updateMessage(withUpdate('available', { installable: true }).update), 'AI Usage v1.11.0 is ready to install.');
  assert.match(updateMessage(withUpdate('available').update), /release page/);
  assert.equal(updateMessage(withUpdate('failed', { error: 'offline' }).update), "Couldn't update: offline");
  assert.equal(updateMessage(withUpdate('failed', { error: 'HTTP 503', version: '' }).update), "Couldn't check: HTTP 503");
  assert.equal(updateMessage(null), '');
  assert.equal(parseHostPayload({ update: { state: 'available', installable: 'yes' } }).update.installable, false);
  assert.equal(parseHostPayload({ update: { state: 'available', installable: true } }).update.installable, true);

  assert.equal(updateModeLabel('auto'), 'Automatic');
  assert.equal(updateModeLabel('notify'), 'Notify me');
  assert.equal(updateModeLabel('off'), 'Off');
  assert.equal(updateModeLabel('whenever'), 'Notify me');
  assert.equal(updateModeLabel(undefined), 'Notify me');
}

// --- SuperGrok labels / displayPlan equality ----------------------------------

{
  // ARRANGE: SuperGrok names the overall meter "<Window> usage" (older
  // reports used "<Window> Build credits"); Grok proper does not.
  const grok = parseHostPayload({
    entries: [
      { id: 'supergrok', display_name: 'SuperGrok', plan: 'SuperGrok', sections: [
        { type: 'metric', label: 'Weekly Build credits', percent: 12, severity: 'low' },
        { type: 'metric', label: 'Monthly Build credits', percent: 3, severity: 'low' },
        { type: 'metric', label: 'Build credits', percent: 3, severity: 'low' },
      ] },
      { id: 'supergrok@work', display_name: 'SuperGrok', plan: 'SuperGrok Heavy', sections: [
        { type: 'metric', label: 'Weekly Build credits', percent: 1, severity: 'low' },
      ] },
      { id: 'grok', display_name: 'Grok', sections: [
        { type: 'metric', label: 'Weekly Build credits', percent: 1, severity: 'low' },
      ] },
    ],
  });
  // ACT
  const [personal, work, plain] = projectCards(grok, 0);
  // ASSERT: only the trailing " Build credits" goes, only for supergrok
  assert.deepEqual(personal.rows.map((r) => r.label), ['Weekly', 'Monthly', 'Build credits']);
  assert.deepEqual(personal.rows.map(rowKey), ['metric:Weekly', 'metric:Monthly', 'metric:Build credits']);
  assert.deepEqual(work.rows.map((r) => r.label), ['Weekly']);
  assert.deepEqual(plain.rows.map((r) => r.label), ['Weekly Build credits']);

  const usageNamed = parseHostPayload({
    entries: [
      { id: 'supergrok', display_name: 'SuperGrok', plan: 'SuperGrok', sections: [
        { type: 'metric', label: 'Weekly usage', percent: 90, severity: 'critical' },
        { type: 'metric', label: 'Grok Build', percent: 87, severity: 'high' },
        { type: 'metric', label: 'Grok Chat', percent: 3, severity: 'low' },
      ] },
    ],
  });
  const [usageCard] = projectCards(usageNamed, 0);
  assert.deepEqual(usageCard.rows.map((r) => r.label), ['Weekly', 'Grok Build', 'Grok Chat']);

  // ASSERT: a plan equal to the title vanishes; a prefixed plan keeps its tail
  assert.equal(displayPlan(personal.title, personal.plan), '');
  assert.equal(displayPlan(work.title, work.plan), 'Heavy');
  assert.equal(displayPlan('SuperGrok', 'supergrok'), '');
  assert.equal(displayPlan('Claude', 'Claude'), '');
  assert.equal(displayPlan('Claude', 'Claude Max 5x'), 'Max 5x');
  assert.equal(displayPlan('', 'Plus'), 'Plus');
}

// --- quotaAlternate ----------------------------------------------------------

assert.equal(quotaAlternate({ kind: 'metric', leftPercent: 81, usedPercent: 19 }, 'left'), '19% used');
assert.equal(quotaAlternate({ kind: 'metric', leftPercent: 81, usedPercent: 19 }, 'used'), '81% left');
assert.equal(quotaAlternate({ kind: 'metric', leftPercent: 0, usedPercent: 100 }, 'left'), '');
assert.equal(quotaAlternate({ kind: 'metric', leftPercent: 0, usedPercent: 100 }, 'used'), '');
assert.equal(quotaAlternate({ kind: 'text', label: 'Balance', value: '$1' }, 'left'), '');
assert.equal(quotaAlternate(null, 'left'), '');

// --- first launch: lacksCredentials / seedLayout / hintPending ---------------

{
  // ARRANGE: two providers with keys, two without, one with an expired login
  const keyed = { id: 'anthropic', error: '' };
  const expired = { id: 'openai', error: 'HTTP 401: authentication rejected' };
  const zai = { id: 'zai', error: 'no API key configured for zai' };
  const openrouter = { id: 'openrouter', error: 'No API key' };
  const entries = [keyed, expired, zai, openrouter];

  // ACT
  const seeded = seedLayout(emptyLayout(), entries);

  // ASSERT: only the "No API key" providers start hidden, once
  assert.equal(lacksCredentials(zai), true);
  assert.equal(lacksCredentials(expired), false);
  assert.equal(lacksCredentials(keyed), false);
  assert.equal(lacksCredentials(null), false);
  assert.deepEqual(seeded.hidden, { zai: true, openrouter: true });
  assert.equal(seeded.seeded, true);
  assert.equal(hintPending(seeded), true);

  // ACT: the user re-enables Z.AI later; a second payload must not hide it again
  const reenabled = { ...seeded, hidden: { openrouter: true } };
  assert.deepEqual(seedLayout(reenabled, entries).hidden, { openrouter: true });
  assert.strictEqual(seedLayout(reenabled, entries), reenabled);

  // ASSERT: no entries (host error) is not a first launch
  assert.equal(seedLayout(emptyLayout(), []).seeded, false);
  assert.equal(seedLayout(emptyLayout(), null).seeded, false);
  assert.equal(hintPending(emptyLayout()), false);

  // ASSERT: when every provider lacks a credential the starter set stays visible
  const allMissing = seedLayout(emptyLayout(), [zai, openrouter]);
  assert.deepEqual(allMissing.hidden, {});
  assert.equal(allMissing.seeded, true);

  // ASSERT: the flags survive storage and syncLayout; the hint can be dismissed
  const store = memoryStorage();
  saveLayout(store, { ...seeded, hintDismissed: true });
  const loaded = loadLayout(store);
  assert.equal(loaded.seeded, true);
  assert.equal(loaded.hintDismissed, true);
  assert.equal(hintPending(loaded), false);
  const synced = syncLayout(loaded, ['anthropic', 'openai', 'zai', 'openrouter']);
  assert.equal(synced.seeded, true);
  assert.equal(synced.hintDismissed, true);
  assert.deepEqual(synced.hidden, { zai: true, openrouter: true });
  assert.equal(emptyLayout().seeded, false);
  assert.equal(emptyLayout().hintDismissed, false);
  assert.equal(normalizeLayout({ seeded: 'yes', hintDismissed: 1 }).seeded, false);
}

// --- absorbPayload ------------------------------------------------------------

{
  // ARRANGE: a remembered layout with hidden providers and a custom order
  const remembered = {
    ...emptyLayout(),
    cardOrder: ['openai', 'anthropic'],
    hidden: { zai: true },
    seeded: true,
    hintDismissed: true,
  };

  // ACT + ASSERT: the host's empty placeholder payload leaves it untouched
  assert.strictEqual(absorbPayload(remembered, []), remembered);
  assert.strictEqual(absorbPayload(remembered, null), remembered);

  // ACT + ASSERT: a real payload syncs (keeps hidden for known ids, appends new ids)
  const entries = [{ id: 'anthropic', error: '' }, { id: 'openai', error: '' }, { id: 'zai', error: 'no API key' }, { id: 'cursor', error: '' }];
  const synced = absorbPayload(remembered, entries);
  assert.deepEqual(synced.cardOrder, ['openai', 'anthropic', 'zai', 'cursor']);
  assert.deepEqual(synced.hidden, { zai: true });
  assert.equal(synced.seeded, true);

  // ACT + ASSERT: a fresh layout gets seeded by its first real payload
  const fresh = absorbPayload(emptyLayout(), entries);
  assert.deepEqual(fresh.hidden, { zai: true });
  assert.equal(fresh.seeded, true);
  assert.equal(hintPending(fresh), true);
}

// --- grouped metrics (Antigravity Session / Weekly) and duplicate labels -----

{
  // ARRANGE: two groups repeating the same metric names, plus a trailing text row
  const grouped = parseHostPayload({
    entries: [{
      id: 'antigravity',
      display_name: 'Antigravity',
      sections: [
        { type: 'spacer' },
        { type: 'text', label: 'Session', value: '' },
        { type: 'metric', label: 'Gemini', percent: 1, severity: 'low' },
        { type: 'metric', label: 'Claude & GPT OSS', percent: 0, severity: 'low' },
        { type: 'text', label: 'Weekly', value: '' },
        { type: 'metric', label: 'Gemini', percent: 4, severity: 'low' },
        { type: 'metric', label: 'Claude & GPT OSS', percent: 0, severity: 'low' },
        { type: 'text', label: 'Warning', value: 'credentials error: no local server found.' },
      ],
    }],
  });

  // ACT
  const card = projectCards(grouped, 0)[0];

  // ASSERT: headings vanish, metrics carry their group, keys stay unique
  assert.deepEqual(card.rows.map((r) => r.label), [
    'Gemini (Session)', 'Claude & GPT OSS (Session)', 'Gemini (Weekly)', 'Claude & GPT OSS (Weekly)',
  ]);
  // the Warning section is the card's warning, not a row
  assert.equal(card.rows.filter((r) => r.kind === 'text').length, 0);
  assert.equal(card.warning.title, "Antigravity isn't running");
  assert.equal(new Set(card.rows.map(rowKey)).size, card.rows.length);
  assert.equal(visibleRowsFor(card, { collapsed: false }).length, 4);
  assert.equal(metricCount(card), 4);

  // ASSERT: a vendor repeating a label outright still yields distinct rows
  const repeated = parseHostPayload({
    entries: [{ id: 'x', display_name: 'X', sections: [
      { type: 'metric', label: 'Quota', percent: 10, severity: 'low' },
      { type: 'metric', label: 'Quota', percent: 20, severity: 'low' },
    ] }],
  });
  const keys = projectCards(repeated, 0)[0].rows.map(rowKey);
  assert.deepEqual(keys, ['metric:Quota', 'metric:Quota #2']);
  assert.equal(visibleRowsFor(projectCards(repeated, 0)[0], { collapsed: false }).length, 2);
}

// --- vendor warnings become card.warning, and errors carry an action ------------

{
  // ARRANGE: cached data plus the Rust "Warning" row, and a Kimi-style warning label
  const warned = parseHostPayload({
    entries: [
      { id: 'antigravity', display_name: 'Antigravity', sections: [
        { type: 'metric', label: 'Gemini', percent: 4, severity: 'low' },
        { type: 'text', label: 'Warning', value: 'credentials error: Antigravity: no local server found. Quota is only served while Antigravity is running.' },
      ] },
      { id: 'kimi', display_name: 'Kimi', sections: [
        { type: 'metric', label: 'Weekly', percent: 10, severity: 'low' },
        { type: 'text', label: 'Kimi API schema drift', value: '' },
      ] },
    ],
  });

  // ACT
  const [agy, kimi] = projectCards(warned, 0);

  // ASSERT: no Warning row; the card carries a translated warning with the cleaned raw text
  assert.deepEqual(agy.rows.map((r) => r.label), ['Gemini']);
  assert.equal(agy.warning.title, "Antigravity isn't running");
  assert.equal(agy.warning.hint, 'Open the Antigravity app or an agy session, then Refresh.');
  assert.ok(!agy.warning.raw.startsWith('credentials error'));
  assert.ok(agy.warning.raw.includes('no local server found'));
  assert.deepEqual(kimi.rows.map((r) => r.label), ['Weekly']);
  assert.equal(kimi.warning.title, "Couldn't update");
  assert.equal(cards[0].warning, null);

  // ASSERT: actions by error class
  assert.deepEqual(explainError('no API key', 'zai').action, { cmd: 'open-tui', label: 'Open TUI' });
  assert.deepEqual(explainError('no vendors enabled').action, { cmd: 'open-tui', label: 'Open TUI' });
  assert.deepEqual(explainError('HTTP 503: down', 'zai').action, { cmd: 'refresh', label: 'Refresh' });
  assert.deepEqual(explainError('network transport error: connection refused', 'openai').action, { cmd: 'refresh', label: 'Refresh' });
  assert.equal(explainError('HTTP 401: authentication rejected', 'openai').action, undefined);
  assert.equal(explainError('HTTP 429: rate limited; next attempt in 4m', 'zai').action, undefined);
  assert.equal(explainError('Antigravity must be running', 'antigravity').title, "Antigravity isn't running");
}

// --- 429 backoff hint --------------------------------------------------------

assert.equal(
  explainError('HTTP 429: rate limited; next attempt in 4m', 'zai').hint,
  'Retrying automatically in 4m.',
);
assert.equal(
  explainError('HTTP 429: rate limited; next attempt in 1h 2m', 'zai').hint,
  'Retrying automatically in 1h 2m.',
);
assert.equal(explainError('HTTP 429: Rate limited. Please try again later.', 'zai').hint, 'Try Refresh in a minute.');

// --- headlineLabel / headlineAlternate ----------------------------------------

assert.equal(headlineLabel({ kind: 'metric', leftPercent: 81, usedPercent: 19 }, 'left'), '81% left');
assert.equal(headlineLabel({ kind: 'metric', leftPercent: 81, usedPercent: 19 }, 'used'), '19% used');
assert.equal(headlineLabel({ kind: 'metric', leftPercent: 0, usedPercent: 100 }, 'left'), '0% left');
assert.equal(headlineLabel({ kind: 'metric', leftPercent: 0, usedPercent: 100 }, 'used'), '100% used');
assert.equal(headlineLabel({ kind: 'text', label: 'Balance', value: '$1' }, 'left'), '');
assert.equal(headlineAlternate({ kind: 'metric', leftPercent: 0, usedPercent: 100 }, 'left'), '100% used');
assert.equal(headlineAlternate({ kind: 'metric', leftPercent: 81, usedPercent: 19 }, 'used'), '81% left');
assert.equal(headlineAlternate(null, 'left'), '');

// A metric that names `value` as its headline draws the money figure, like the
// Omarchy bar; the percentage and the report's detail move to the hover text.
// `percent` keeps the used/left toggle, and an older report without the field
// is a percentage.
const tank = (headline) => projectCards(parseHostPayload({
  version: '1.21.0',
  entries: [{
    id: 'deepseek',
    display_name: 'DeepSeek',
    sections: [{
      type: 'metric',
      label: 'Balance',
      percent: 40,
      value: headline === 'value' ? '$12.00' : '40%',
      detail: headline === 'value' ? '40% of $20.00 used ($12.00 left)' : '$12.00 of $20.00 left (40% used)',
      severity: 'low',
      ...(headline ? { headline } : {}),
    }],
  }],
}), 0)[0].rows[0];
const amountRow = tank('value');
assert.equal(amountRow.headline, 'value');
assert.equal(headlineLabel(amountRow, 'left'), '$12.00');
assert.equal(headlineLabel(amountRow, 'used'), '$12.00');
assert.equal(headlineAlternate(amountRow, 'used'), '40% used · 40% of $20.00 used ($12.00 left)');
assert.equal(headlineAlternate(amountRow, 'left'), '60% left · 40% of $20.00 used ($12.00 left)');
const percentRow = tank('percent');
assert.equal(percentRow.headline, 'percent');
assert.equal(headlineLabel(percentRow, 'used'), '40% used');
assert.equal(headlineAlternate(percentRow, 'used'), '60% left');
assert.equal(tank(undefined).headline, 'percent');
assert.equal(headlineLabel(tank(undefined), 'left'), '60% left');
// A `value` headline with no value to draw falls back to the percentage
// rather than an empty button.
assert.equal(projectCards(parseHostPayload({
  version: '1.21.0',
  entries: [{ id: 'deepseek', sections: [{ type: 'metric', label: 'Balance', percent: 40, value: '', headline: 'value' }] }],
}), 0)[0].rows[0].headline, 'percent');

// --- condensedTextRowIndexes -------------------------------------------------

assert.deepEqual(
  condensedTextRowIndexes([{ kind: 'metric' }, { kind: 'text' }, { kind: 'text' }]),
  [2],
);
assert.deepEqual(
  condensedTextRowIndexes([{ kind: 'text' }, { kind: 'block' }, { kind: 'metric' }, { kind: 'text' }]),
  [1],
);
assert.deepEqual(condensedTextRowIndexes([{ kind: 'text' }]), []);
assert.deepEqual(condensedTextRowIndexes([{ kind: 'metric' }, { kind: 'metric' }]), []);
assert.deepEqual(condensedTextRowIndexes([]), []);
assert.deepEqual(condensedTextRowIndexes(null), []);
assert.deepEqual(condensedTextRowIndexes('nope'), []);

// --- providerIconId / initialsGlyph ------------------------------------------

assert.equal(providerIconId('anthropic@work'), 'anthropic');
assert.equal(providerIconId('supergrok'), 'grok');
assert.equal(providerIconId('SuperGrok@personal'), 'grok');
assert.equal(providerIconId('grokbot'), 'grokbot');
assert.equal(providerIconId(' OpenAI '), 'openai');
assert.equal(providerIconId(''), '');
assert.equal(providerIconId(undefined), '');

assert.equal(initialsGlyph('Claude'), 'CL');
assert.equal(initialsGlyph('  codex'), 'CO');
assert.equal(initialsGlyph('x'), 'X');
assert.equal(initialsGlyph(''), '?');
assert.equal(initialsGlyph('   '), '?');
assert.equal(initialsGlyph(undefined), '?');

// --- metricCount -------------------------------------------------------------

assert.equal(metricCount(cursorCard), 3);
assert.equal(metricCount(cards[0]), 1);
assert.equal(metricCount({ rows: [] }), 0);
assert.equal(metricCount({}), 0);
assert.equal(metricCount(null), 0);

// --- sendCommand merges extra fields -----------------------------------------

{
  // ARRANGE: capture what the host would receive
  const sent = [];
  globalThis.window = { ipc: { postMessage(msg) { sent.push(msg); } } };
  try {
    // ACT
    sendCommand('resize', { height: 512, theme: 'dark' });
    sendCommand('refresh');
    sendCommand('quit', ['not', 'an', 'object']);
    sendCommand('open', { cmd: 'hijack', tab: 'settings' });
    // ASSERT: cmd comes first, extras follow, non-objects are ignored, cmd can't be overridden
    assert.deepEqual(sent.map((msg) => JSON.parse(msg)), [
      { cmd: 'resize', height: 512, theme: 'dark' },
      { cmd: 'refresh' },
      { cmd: 'quit' },
      { cmd: 'open', tab: 'settings' },
    ]);
    assert.equal(sent[0], '{"cmd":"resize","height":512,"theme":"dark"}');
  } finally {
    delete globalThis.window;
  }
}

{
  // ARRANGE: no host bridge at all
  // ACT / ASSERT: sending is a no-op rather than a throw
  assert.doesNotThrow(() => sendCommand('refresh', { x: 1 }));
}

// --- resolvedTheme --------------------------------------------

assert.equal(resolvedTheme('dark'), 'dark');
assert.equal(resolvedTheme('light'), 'light');
{
  // ARRANGE: an OS that prefers dark
  globalThis.window = { matchMedia: () => ({ matches: true }) };
  try {
    // ACT / ASSERT
    assert.equal(resolvedTheme('system'), 'dark');
    assert.equal(resolvedTheme('bogus'), 'dark');
  } finally {
    delete globalThis.window;
  }
}
{
  // ARRANGE: an OS that prefers light
  globalThis.window = { matchMedia: () => ({ matches: false }) };
  try {
    assert.equal(resolvedTheme('system'), 'light');
  } finally {
    delete globalThis.window;
  }
}
// without a window, "system" resolves to light
assert.equal(resolvedTheme('system'), 'light');

{
  const layout = emptyLayout();
  assert.equal(layout.stripStyle, 'bars');
  assert.deepEqual(layout.stars, {});
  const restored = normalizeLayout({
    stripStyle: 'text',
    stars: { anthropic: ['metric:Weekly', 'metric:Session', 'metric:Extra'] },
  });
  assert.equal(restored.stripStyle, 'bars');
  assert.deepEqual(restored.stars.anthropic, ['metric:Weekly', 'metric:Session']);
  assert.equal(MAX_STARS_PER_PROVIDER, 2);
}

{
  const payload = parseHostPayload({
    entries: [{
      id: 'anthropic',
      display_name: 'Claude',
      sections: [
        { type: 'metric', label: 'Weekly', percent: 19 },
        { type: 'metric', label: 'Session', percent: 41 },
        { type: 'text', label: 'Extra', value: '$1' },
      ],
    }],
  });
  const cards = projectCards(payload, 0);
  const stars = defaultStars(cards);
  assert.deepEqual(stars.anthropic, ['metric:Weekly', 'metric:Session']);
  assert.equal(isStarred(stars, 'anthropic', 'metric:Weekly'), true);
  const added = toggleStar(stars, 'anthropic', 'metric:Weekly');
  assert.equal(isStarred(added.stars, 'anthropic', 'metric:Weekly'), false);
  assert.equal(added.error, '');
  const capped = toggleStar(stars, 'anthropic', 'text:Extra');
  assert.equal(capped.error, 'Up to 2 stars per provider');
  assert.deepEqual(capped.stars, stars);
  const seeded = seedStars(emptyLayout(), cards);
  assert.deepEqual(seeded.stars, stars);
  const kept = seedStars(seeded, cards);
  assert.equal(kept, seeded);
  assert.deepEqual(stripCommand(seeded, cards), {
    style: 'bars',
    stars,
    order: ['anthropic'],
  });
}

{
  const payload = parseHostPayload({
    entries: [
      { id: 'anthropic', display_name: 'Claude', sections: [{ type: 'metric', label: 'Weekly', percent: 10 }] },
      { id: 'openai', display_name: 'Codex', sections: [{ type: 'metric', label: 'Codex weekly', percent: 2 }] },
    ],
  });
  const cards = projectCards(payload, 0);
  const layout = {
    ...emptyLayout(),
    cardOrder: ['openai', 'anthropic'],
    stars: {
      anthropic: ['metric:Weekly'],
      openai: ['metric:Codex weekly'],
    },
  };
  assert.deepEqual(stripCommand(layout, cards).order, ['openai', 'anthropic']);
}

{
  assert.equal(expirySeverity(1_000 + 8 * 24 * 3600 * 1000, 1_000), 'blue');
  assert.equal(expirySeverity(1_000 + 3 * 24 * 3600 * 1000, 1_000), 'yellow');
  assert.equal(expirySeverity(1_000 + 2 * 3600 * 1000, 1_000), 'red');
  assert.equal(expirySeverity(500, 1_000), 'red');
  // A host that sends `reset_credits` without the text block still gets the
  // row, and it starts On Demand like every non-metric row.
  const payload = parseHostPayload({
    entries: [{
      id: 'openai',
      display_name: 'Codex',
      reset_credits: { available: 2, credits: [{ expires_at: '2026-10-03T23:00:00Z' }] },
      sections: [
        { type: 'metric', label: 'Weekly', percent: 10, reset_at: '2026-10-01T00:00:00Z' },
      ],
    }],
  });
  const [card] = projectCards(payload, 0);
  assert.equal(card.resetCredits.available, 2);
  const resets = card.rows.find((row) => row.kind === 'resetCredits');
  assert.equal(resets.label, 'Rate Limit Resets');
  assert.equal(resets.available, 2);
  assert.equal(resets.credits.length, 1);
  assert.equal(card.rows.filter((row) => row.kind === 'resetCredits').length, 1);
  const prefs = defaultRowPrefs(card.rows);
  assert.ok(prefs.demand.indexOf('resetCredits:Rate Limit Resets') >= 0);
  assert.ok(prefs.always.indexOf('resetCredits:Rate Limit Resets') < 0);
}

{
  assert.equal(isHttpUrl('https://status.anthropic.com/'), true);
  assert.equal(isHttpUrl('javascript:alert(1)'), false);
  const claude = providerLinks('anthropic');
  assert.equal(claude.length, 2);
  assert.equal(claude[0].label, 'Status');
  assert.equal(claude[1].label, 'Dashboard');
  const codex = providerLinks('openai@work');
  assert.equal(codex[0].label, 'Status');
  assert.equal(codex[1].label, 'Dashboard');
  assert.equal(providerLinks('unknown-vendor').length, 0);
  const grok = providerLinks('supergrok');
  assert.equal(grok.length, 1);
  assert.equal(grok[0].label, 'Usage');
  assert.ok(grok[0].url.indexOf('https://grok.com/') === 0);
}

{
  assert.equal(prettyMetricLabel('anthropic', 'Session (5h)'), 'Session');
  assert.equal(prettyMetricLabel('anthropic', 'Weekly (7d)'), 'Weekly');
  assert.equal(prettyMetricLabel('anthropic', 'Fable (7d)'), 'Fable');
  assert.equal(prettyMetricLabel('openai', 'Codex weekly'), 'Weekly');
  assert.equal(prettyMetricLabel('openai', 'Codex 5h'), 'Session');
  assert.equal(prettyMetricLabel('cursor', 'Cursor Models'), 'Cursor Models');
  assert.equal(prettyMetricLabel('antigravity', 'Gemini', 'Session'), 'Gemini (Session)');
  const payload = parseHostPayload({
    entries: [{
      id: 'openai',
      display_name: 'Codex',
      sections: [
        { type: 'metric', label: 'Codex weekly', percent: 28 },
        { type: 'metric', label: 'Codex 5h', percent: 0 },
      ],
    }],
  });
  const [card] = projectCards(payload, 0);
  assert.equal(card.rows[0].label, 'Weekly');
  assert.equal(card.rows[0].key, 'metric:Codex weekly');
  assert.equal(card.rows[1].label, 'Session');
  assert.equal(card.rows[1].key, 'metric:Codex 5h');
}

// Account switch: only the macOS host reports switchable logins, keyed by
// vendor; each named card finds its own label and nothing else does.
{
  const payload = parseHostPayload(JSON.stringify({
    entries: [],
    accounts: {
      anthropic: { active: 'main', labels: ['main', 'work'], target: 'work', switching: false, error: 'no stored credential' },
      openai: { active: '', labels: ['main', 'work'], target: 'work', switching: true, error: '' },
      cursor: { active: 'x', labels: ['x'] },
      grok: 'not an object',
    },
  }));
  assert.deepEqual(Object.keys(payload.accounts).sort(), ['anthropic', 'openai']);

  const active = accountSwitchFor('anthropic@main', payload.accounts);
  assert.equal(active.active, true);
  assert.equal(active.error, '');

  const failed = accountSwitchFor('anthropic@work', payload.accounts);
  assert.equal(failed.active, false);
  assert.equal(failed.error, 'no stored credential');

  const running = accountSwitchFor('openai@work', payload.accounts);
  assert.equal(running.switching, true);
  assert.equal(running.busy, false);
  const waiting = accountSwitchFor('openai@main', payload.accounts);
  assert.equal(waiting.busy, true);
  assert.equal(waiting.switching, false);

  assert.equal(accountSwitchFor('anthropic', payload.accounts), null);
  assert.equal(accountSwitchFor('anthropic@unknown', payload.accounts), null);
  assert.equal(accountSwitchFor('cursor@x', payload.accounts), null);
  assert.equal(accountSwitchFor('openai@work', {}), null);
  assert.deepEqual(parseHostPayload(JSON.stringify({ entries: [] })).accounts, {});
}

// Every card that renders keeps its switch control: as many accounts as there
// are cards, and a label longer than a card id matched through the id's cut.
{
  const many = Array.from({ length: 63 }, (_, i) => `acct${i}`);
  const long = 'x'.repeat(200);
  const payload = parseHostPayload(JSON.stringify({
    entries: [{ id: `openai@${long}` }],
    accounts: {
      openai: { active: long, labels: [...many, long] },
      anthropic: { active: '', labels: [`${long}-a`, `${long}-b`] },
    },
  }));
  assert.equal(accountSwitchFor('openai@acct62', payload.accounts).label, 'acct62');
  const cardId = payload.entries[0].id;
  assert.notEqual(cardId, `openai@${long}`);
  const control = accountSwitchFor(cardId, payload.accounts);
  assert.equal(control.label, long);
  assert.equal(control.active, true);
  // Two labels that share the cut id are ambiguous, so neither card offers one.
  assert.equal(accountSwitchFor(cardId.replace('openai', 'anthropic'), payload.accounts), null);
}

// Early in a window there is no pace yet: the meter says "Estimating…". The wait
// is 1% of the window, capped at an hour so long windows do not sit for hours.
{
  const now = Date.parse('2026-09-23T15:00:00Z');
  const week = 604800;
  const at = (window, elapsedSecs, used = 1) => ({
    usedPercent: used,
    resetAt: new Date(now + (window - elapsedSecs) * 1000).toISOString(),
    window,
  });
  const young = at(week, 1800); // 30m into a week
  assert.equal(pace(young, now), null);
  assert.equal(paceWarmupMs(young, now), 1800_000);
  assert.equal(paceWarmupText(young, now), 'Estimating…');
  assert.equal(paceWarmupHint(young), 'The pace shows after the first 1h of each window.');
  // A week or a month waits an hour, not 1h 41m or 7h 12m.
  assert.notEqual(pace(at(week, 3600), now), null);
  assert.notEqual(pace(at(2592000, 3600), now), null);
  // A 5h session keeps its 3 minutes.
  assert.equal(pace(at(18000, 170), now), null);
  assert.equal(paceWarmupHint(at(18000, 170)), 'The pace shows after the first 3m of each window.');
  // Nothing spent, no window, or already projecting: no warm-up note.
  assert.equal(paceWarmupText({ ...young, usedPercent: 0 }, now), '');
  assert.equal(paceWarmupText({ ...young, window: 0 }, now), '');
  assert.equal(paceWarmupText(at(week, 7200), now), '');
}

// Star keys are a contract with the tray host (src/tray/strip.rs metric_key),
// which reads the same fixture: a rule changed here alone fails its test there.
{
  const fixture = JSON.parse(readFileSync(new URL('../../tests/fixtures/strip_metric_keys.json', import.meta.url), 'utf8'));
  assert.ok(fixture.cases.length > 0);
  for (const c of fixture.cases) {
    assert.equal(metricRowKey(c.id, c.label, c.group || ''), c.key, JSON.stringify(c));
  }
}

console.log('ok');
