import assert from 'node:assert/strict';
import {
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
  condensedTextRowIndexes,
  providerIconId,
  initialsGlyph,
  metricCount,
  sendCommand,
  resolvedTheme,
  applyDensity,
  emptyPayload,
  pace,
  paceText,
  paceTickPercent,
  paceVisible,
  shortcutFromKeyEvent,
  formatAgo,
  updateStatusLabel,
  updateBannerPending,
  updateModeLabel,
} from './src/model.js';

const report = {
  version: '1.10.0',
  generated_at: 1_000,
  next_refresh_at: 61_000,
  startup_enabled: true,
  host_error: null,
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
    stale: true,
    sections: [],
  }],
});
const errorCards = projectCards(errored, 0);
assert.equal(errorCards[0].errorTitle, 'Sign-in expired');
assert.equal(errorCards[0].errorHint, 'Run codex login in a terminal, then Refresh.');
assert.equal(errorCards[0].error, 'Sign-in expired. Run codex login in a terminal, then Refresh.');
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
    sections: [],
  }],
});
const claudeCards = projectCards(claudeErrored, 0);
assert.equal(claudeCards[0].title, 'Claude');
assert.equal(claudeCards[0].plan, 'Claude Max 5x');
assert.equal(claudeCards[0].errorTitle, 'Sign-in expired');
assert.equal(claudeCards[0].errorHint, 'Run claude in a terminal, then Refresh.');
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

const extraOnAlways = moveRowToList(
  defaultRowPrefs(cursorCard.rows),
  'text:Extra Usage',
  'always',
  null,
);
assert.deepEqual(extraOnAlways.always.slice(-1), ['text:Extra Usage']);
assert.equal(visibleRowsFor(cursorCard, { collapsed: true, prefs: extraOnAlways }).length, 3);
const disabledApi = setRowEnabled(defaultRowPrefs(cursorCard.rows), 'metric:API', false);
assert.equal(disabledApi.off['metric:API'], true);
assert.equal(visibleRowsFor(cursorCard, { collapsed: false, prefs: disabledApi }).length, 3);
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

// --- density / resetTimes layout fields ------------------------------------

{
  // ARRANGE: a fresh layout
  // ACT
  const empty = emptyLayout();
  // ASSERT: defaults are regular density and countdown reset times
  assert.equal(empty.density, 'regular');
  assert.equal(empty.resetTimes, 'countdown');
}

{
  // ARRANGE: valid and invalid values for the new fields
  // ACT
  const compactExact = normalizeLayout({ density: 'compact', resetTimes: 'exact' });
  const junk = normalizeLayout({ density: 'tiny', resetTimes: 'never' });
  const missing = normalizeLayout({});
  // ASSERT: only the two known values survive; everything else falls back
  assert.equal(compactExact.density, 'compact');
  assert.equal(compactExact.resetTimes, 'exact');
  assert.equal(junk.density, 'regular');
  assert.equal(junk.resetTimes, 'countdown');
  assert.equal(missing.density, 'regular');
  assert.equal(missing.resetTimes, 'countdown');
}

{
  // ARRANGE: a saved layout with the new fields set
  const densityStore = memoryStorage();
  saveLayout(densityStore, { cardOrder: ['cursor'], density: 'compact', resetTimes: 'exact' });
  // ACT: reload and sync against the live card set
  const reloaded = loadLayout(densityStore);
  const resynced = syncLayout(reloaded, ['anthropic', 'cursor']);
  // ASSERT: both fields round-trip through storage and syncLayout
  assert.equal(reloaded.density, 'compact');
  assert.equal(reloaded.resetTimes, 'exact');
  assert.equal(resynced.density, 'compact');
  assert.equal(resynced.resetTimes, 'exact');
  assert.deepEqual(resynced.cardOrder, ['cursor', 'anthropic']);
}

{
  // ARRANGE: a layout with bogus values for the new fields
  // ACT
  const cleaned = syncLayout({ cardOrder: [], density: 'huge', resetTimes: 'maybe' }, ['anthropic']);
  // ASSERT: syncLayout normalizes rather than passing junk through
  assert.equal(cleaned.density, 'regular');
  assert.equal(cleaned.resetTimes, 'countdown');
}

// --- meterColor --------------------------------------------------------------

assert.equal(meterColor('low'), 'blue');
assert.equal(meterColor('mid'), 'yellow');
assert.equal(meterColor('high'), 'yellow');
assert.equal(meterColor('critical'), 'red');
assert.equal(meterColor('nope'), 'blue');
assert.equal(meterColor(undefined), 'blue');

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

// --- layout: timeFormat / alwaysShowPace ------------------------------------

{
  // ARRANGE / ACT
  const empty = emptyLayout();
  const set = normalizeLayout({ timeFormat: '24', alwaysShowPace: true });
  const junk = normalizeLayout({ timeFormat: 'military', alwaysShowPace: 'yes' });
  // ASSERT: defaults, valid values, and junk
  assert.equal(empty.timeFormat, 'auto');
  assert.equal(empty.alwaysShowPace, false);
  assert.equal(set.timeFormat, '24');
  assert.equal(set.alwaysShowPace, true);
  assert.equal(junk.timeFormat, 'auto');
  assert.equal(junk.alwaysShowPace, false);
  assert.equal(normalizeLayout({ timeFormat: '12' }).timeFormat, '12');

  // ASSERT: both survive storage and syncLayout
  const store = memoryStorage();
  saveLayout(store, { cardOrder: ['cursor'], timeFormat: '12', alwaysShowPace: true });
  const reloaded = loadLayout(store);
  assert.equal(reloaded.timeFormat, '12');
  assert.equal(reloaded.alwaysShowPace, true);
  const synced = syncLayout(reloaded, ['cursor']);
  assert.equal(synced.timeFormat, '12');
  assert.equal(synced.alwaysShowPace, true);
  const cleaned = syncLayout({ cardOrder: [], timeFormat: 'nope', alwaysShowPace: 1 }, ['cursor']);
  assert.equal(cleaned.timeFormat, 'auto');
  assert.equal(cleaned.alwaysShowPace, false);
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
  assert.equal(updateBannerPending(null), false);

  assert.equal(updateModeLabel('auto'), 'Automatic');
  assert.equal(updateModeLabel('notify'), 'Notify me');
  assert.equal(updateModeLabel('off'), 'Off');
  assert.equal(updateModeLabel('whenever'), 'Notify me');
  assert.equal(updateModeLabel(undefined), 'Notify me');
}

// --- SuperGrok labels / displayPlan equality ----------------------------------

{
  // ARRANGE: SuperGrok names every meter "<Window> Build credits"; Grok proper does not
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

// --- resolvedTheme / applyDensity --------------------------------------------

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
  // ARRANGE: a minimal document
  globalThis.document = { documentElement: { dataset: {} } };
  try {
    // ACT
    applyDensity('compact');
    assert.equal(globalThis.document.documentElement.dataset.density, 'compact');
    applyDensity('anything-else');
    // ASSERT
    assert.equal(globalThis.document.documentElement.dataset.density, 'regular');
  } finally {
    delete globalThis.document;
  }
}
// no document: no-op
assert.doesNotThrow(() => applyDensity('compact'));

console.log('ok');
