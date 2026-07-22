import assert from 'node:assert/strict';
import {barMarkup, colorForDelta, disambiguateTags, field, FIELD, FORMAT, hasUsageWindows, integer,
    isGrouped, markerElapsed, pickPool, plainTextFromPango, poolTag,
    splitFormatOutput} from './marker-logic.js';

const colors = {low: 'low', mid: 'mid', high: 'high', critical: 'critical', empty: 'empty'};
const visibleCells = markup => markup.replace(/<[^>]+>/g, '');

assert.equal(markerElapsed('1h', 0), 0);
assert.equal(markerElapsed('1h', 100), 100);
assert.equal(markerElapsed('', 0), null);
assert.equal(markerElapsed('—', 0), null);
assert.equal(markerElapsed('1h', null), null);

assert.equal(integer('27'), 27);
assert.equal(integer('27 ⏸'), null);
assert.equal(integer('{scoped_elapsed}'), null);
assert.equal(field('{scoped_model}'), '');
assert.equal(plainTextFromPango('<span>A &amp; B &lt;b&gt;</span>'), 'A & B <b>');
assert.equal(plainTextFromPango('&amp;lt;literal&amp;gt;'), '&lt;literal&gt;');
assert.equal(hasUsageWindows('dsk'), false);
assert.equal(hasUsageWindows('gpt'), true);
assert.equal(hasUsageWindows('agy'), true);
assert.equal(hasUsageWindows('{vendor_short}'), true); // older binary compatibility
const formatFields = FORMAT.split(';;');
assert.deepEqual(formatFields, [
    '{plan}', '{session_pct}', '{session_reset}', '{weekly_pct}', '{weekly_reset}',
    '{sonnet_pct}', '{sonnet_reset}', '{extra_pct}', '{extra_spent}', '{extra_limit}',
    '{scoped_model}', '{scoped_pct}', '{scoped_reset}', '{session_elapsed}',
    '{weekly_elapsed}', '{scoped_elapsed}', '{vendor_short}',
    '{extra_model}', '{extra_reset}', '{extra_elapsed}',
    '{session_model}', '{weekly_model}', '__aiub_end__',
]);
assert.deepEqual(FIELD, {
    plan: 0, sessionPct: 1, sessionReset: 2, weeklyPct: 3, weeklyReset: 4,
    sonnetPct: 5, sonnetReset: 6, extraPct: 7, extraSpent: 8, extraLimit: 9,
    scopedModel: 10, scopedPct: 11, scopedReset: 12,
    sessionElapsed: 13, weeklyElapsed: 14, scopedElapsed: 15, vendorShort: 16,
    extraModel: 17, extraReset: 18, extraElapsed: 19,
    sessionModel: 20, weeklyModel: 21,
    sentinel: 22,
});
// Appended fields must not disturb the indices an older binary already fills.
assert.equal(FIELD.vendorShort, 16);
// An older binary echoes unknown placeholders back; field() must discard them
// so the extra row falls back to its spent/limit money-budget shape.
assert.equal(field('{extra_model}'), '');
assert.equal(field('{extra_reset}'), '');
// Grouped layout is opted into by data, never by vendor id. An older binary
// echoes the placeholder back, which must read as "not grouped".
assert.equal(isGrouped('Gemini'), true);
assert.equal(isGrouped('{session_model}'), false);
assert.equal(isGrouped(''), false);
assert.equal(isGrouped(undefined), false);

// Panel pool tags.
assert.equal(poolTag('Gemini'), 'G');
assert.equal(poolTag('Claude & GPT OSS'), 'C');
assert.equal(poolTag(''), '');
assert.equal(poolTag('{session_model}'), ''); // older binary echoes it back
assert.equal(poolTag(undefined), '');
// Leading punctuation must not become the tag.
assert.equal(poolTag('  &claude'), 'C');

assert.deepEqual(disambiguateTags('Gemini', 'Claude & GPT OSS'), ['G', 'C']);
// Shared initial widens both tags until they differ.
assert.deepEqual(disambiguateTags('Gemini', 'GPT OSS'), ['GE', 'GP']);
assert.deepEqual(disambiguateTags('Gemini Pro', 'Gemini Flash'), ['GEMINIP', 'GEMINIF']);
// Nothing to disambiguate against.
assert.deepEqual(disambiguateTags('Gemini', ''), ['G', '']);

// Auto pool selection. A pool is spent when either window crosses the threshold.
const free = {session: 10, weekly: 10};
const spent5h = {session: 99, weekly: 10};
const spentWeekly = {session: 10, weekly: 99};
assert.equal(pickPool(free, free, 95), 'primary');
assert.equal(pickPool(spent5h, free, 95), 'secondary');
// The weekly running out must switch too, not just the 5h.
assert.equal(pickPool(spentWeekly, free, 95), 'secondary');
// Both spent → stay on the preferred pool instead of flapping.
assert.equal(pickPool(spent5h, spent5h, 95), 'primary');
// Never switch away from a healthy primary.
assert.equal(pickPool(free, spent5h, 95), 'primary');
// The threshold itself counts as spent.
assert.equal(pickPool({session: 95, weekly: 0}, free, 95), 'secondary');
assert.equal(pickPool({session: 94, weekly: 0}, free, 95), 'primary');
// A missing secondary pool reads as 0% and keeps the primary.
assert.equal(pickPool(free, undefined, 95), 'primary');
const values = {'{scoped_elapsed}': '27'};
const framed = splitFormatOutput(formatFields.map(value => values[value] ?? value).join(';;') + ' ⏸');
assert.equal(integer(framed[FIELD.scopedElapsed]), 27);
assert.equal(framed[FIELD.sentinel], '__aiub_end__ ⏸');

assert.equal(colorForDelta(-11, colors), 'low');
assert.equal(colorForDelta(-10, colors), 'mid');
assert.equal(colorForDelta(0, colors), 'mid');
assert.equal(colorForDelta(1, colors), 'high');
assert.equal(colorForDelta(9, colors), 'high');
assert.equal(colorForDelta(10, colors), 'critical');

for (const [pct, elapsed, expected] of [[25, 50, 'low'], [50, 50, 'mid'], [75, 50, 'critical']]) {
    const markup = barMarkup(pct, 8, colors, elapsed);
    assert.equal(visibleCells(markup).length, 8);
    assert.ok(markup.includes('│'));
    assert.ok(markup.includes(`foreground="${expected}"`));
}
assert.equal(visibleCells(barMarkup(50, 8, colors, 0)).length, 8);
assert.equal(visibleCells(barMarkup(50, 8, colors, 100)).length, 8);
assert.ok(!barMarkup(50, 8, colors, markerElapsed('—', 0)).includes('│'));

console.log('marker logic tests passed');
