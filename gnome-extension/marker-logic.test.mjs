import assert from 'node:assert/strict';
import {barMarkup, colorForDelta, field, FIELD, FORMAT, hasUsageWindows, integer, markerElapsed,
    plainTextFromPango, splitFormatOutput} from './marker-logic.js';

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
assert.equal(hasUsageWindows('{vendor_short}'), true); // older binary compatibility
const formatFields = FORMAT.split(';;');
assert.deepEqual(formatFields, [
    '{plan}', '{session_pct}', '{session_reset}', '{weekly_pct}', '{weekly_reset}',
    '{sonnet_pct}', '{sonnet_reset}', '{extra_pct}', '{extra_spent}', '{extra_limit}',
    '{scoped_model}', '{scoped_pct}', '{scoped_reset}', '{session_elapsed}',
    '{weekly_elapsed}', '{scoped_elapsed}', '{vendor_short}', '__aiub_end__',
]);
assert.deepEqual(FIELD, {
    plan: 0, sessionPct: 1, sessionReset: 2, weeklyPct: 3, weeklyReset: 4,
    sonnetPct: 5, sonnetReset: 6, extraPct: 7, extraSpent: 8, extraLimit: 9,
    scopedModel: 10, scopedPct: 11, scopedReset: 12,
    sessionElapsed: 13, weeklyElapsed: 14, scopedElapsed: 15, vendorShort: 16,
    sentinel: 17,
});
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
