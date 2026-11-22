import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {commandFailure, elapsedPercent, errorLine, finitePercent, formatDuration, formatReset, metricDetail,
    parseReport, projectEntry, summarize, brandSlug} from './report-model.js';

const now = Date.parse('2026-09-24T18:00:00Z');
const labels = rows => rows.map(row => `${row.type}:${row.label}`);

{
    const fixture = JSON.parse(readFileSync(new URL('../tests/fixtures/grokbot_paced_report.json', import.meta.url), 'utf8'));
    const entry = projectEntry(fixture.entries[0], Date.parse('2026-09-25T12:00:00Z'));
    assert.equal(entry.title, 'Grok Bot');
    assert.equal(entry.rows[0].elapsed, 50);
    assert.equal(entry.rows[0].detail, '50% elapsed · 20pts ahead');
    const missing = projectEntry({...fixture.entries[0], sections: [{...fixture.entries[0].sections[0], window_secs: undefined}]}, now);
    assert.equal(missing.rows[0].elapsed, null);
}

{
    const fixture = JSON.parse(readFileSync(new URL('../tests/fixtures/cursor_paced_report.json', import.meta.url), 'utf8'));
    const entry = projectEntry(fixture.entries[0], Date.parse('2026-09-25T12:00:00Z'));
    assert.equal(entry.title, 'Cursor');
    assert.deepEqual(entry.rows.map(row => row.elapsed), [50, 50]);
    assert.equal(entry.rows[0].detail, 'Auto + Composer · 50% elapsed · 20pts ahead');
    assert.equal(entry.rows[1].detail, 'Named / API models · on-demand off · 50% elapsed · 20pts under');
    const unstated = projectEntry({...fixture.entries[0], sections: fixture.entries[0].sections.map(section => ({...section, window_secs: undefined}))}, now);
    assert.deepEqual(unstated.rows.map(row => row.elapsed), [null, null]);
}

assert.equal(finitePercent(37), 37);
assert.equal(finitePercent('83'), 83);
assert.equal(finitePercent(null), null);
assert.equal(finitePercent(undefined), null);
assert.equal(finitePercent(''), null);
assert.equal(finitePercent(-1), null);
assert.equal(finitePercent(101), null);
assert.equal(finitePercent(false), null);
assert.equal(finitePercent('   '), null);
assert.equal(finitePercent([]), null);

assert.equal(formatReset('2026-09-24T21:35:00Z', now), '3h 35m');
assert.equal(formatReset('2026-09-24T17:00:00Z', now), 'now');
assert.equal(formatReset('2026-09-24T18:00:30Z', now), '1m');
assert.equal(formatReset('', now), '');
assert.equal(formatReset('not-a-date', now), '');

// The pace marker needs both reset_at and window_secs, and floors like
// src/pacing.rs: 1h 07m left of 5h is 77.6% gone, which Rust reports as 77.
assert.equal(elapsedPercent('2026-09-24T19:07:00Z', 18000, now), 77);
assert.equal(elapsedPercent('2026-09-24T19:07:00Z', undefined, now), null);
assert.equal(elapsedPercent('2026-09-24T19:07:00Z', null, now), null);
assert.equal(elapsedPercent('2026-09-24T19:07:00Z', 0, now), null);
assert.equal(elapsedPercent(null, 18000, now), null);
assert.equal(elapsedPercent('2026-09-24T17:00:00Z', 18000, now), 100);
assert.equal(elapsedPercent('2026-09-25T18:00:00Z', 18000, now), 0);

// The menu draws its own countdown, so the CLI's "Resets in …" goes; what
// else the detail says stays. Without a reset_at there is no countdown, so
// nothing is stripped.
assert.equal(metricDetail('Resets in 1h 07m · 77% elapsed · 7pts ahead', '2026-09-25T00:00:00Z'), '77% elapsed · 7pts ahead');
assert.equal(metricDetail('Resets in 4h 52m', '2026-09-25T00:00:00Z'), '');
assert.equal(metricDetail('Regenerates 2% in 3h 04m · 50% elapsed · 20pts ahead', '2026-09-25T00:00:00Z'), '50% elapsed · 20pts ahead');
assert.equal(metricDetail('Regenerates 2% in 3h', '2026-09-25T00:00:00Z'), '');
assert.equal(metricDetail('Regenerates 20 credits in 3h', '2026-09-25T00:00:00Z'), '');
assert.equal(metricDetail('Auto + Composer', '2026-09-25T00:00:00Z'), 'Auto + Composer');
assert.equal(metricDetail('$4.10 of $20.00 · reset Oct 1', '2026-09-25T00:00:00Z'), '$4.10 of $20.00');
assert.equal(metricDetail('Resets in 3d', ''), 'Resets in 3d');
assert.equal(metricDetail('Regenerates 2% in 3h', ''), 'Regenerates 2% in 3h');

// Shapes below are what `ai-usagebar usage --json` prints (v1.23), trimmed to
// the fields the menu reads.
const claude = projectEntry({
    id: 'anthropic',
    display_name: 'Claude',
    plan: 'Claude Pro',
    sections: [
        {type: 'spacer'},
        {type: 'metric', label: 'Session (5h)', percent: 84, value: '84%', headline: 'percent',
            severity: 'high', detail: 'Resets in 1h 07m · 77% elapsed · 7pts ahead',
            reset_at: '2026-09-24T19:07:00Z', window_secs: 18000},
        {type: 'spacer'},
        {type: 'metric', label: 'Weekly (7d)', percent: 0, value: '0%', headline: 'percent',
            severity: 'low', detail: 'Resets in 6d 23h · 0% elapsed · on track',
            reset_at: '2026-10-01T17:00:00Z', window_secs: 604800},
    ],
}, now);
assert.equal(claude.title, 'Claude');
assert.equal(claude.plan, 'Claude Pro');
assert.deepEqual(labels(claude.rows), ['metric:Session (5h)', 'metric:Weekly (7d)']);

{
    const paced = projectEntry({
        id: 'paced',
        sections: [{type: 'metric', label: 'Weekly (7d)', percent: 27, value: '27%',
            headline: 'percent', severity: 'low', detail: 'Resets in 3d',
            reset_at: '2026-09-28T18:00:00Z', window_secs: 604800,
            elapsed_percent: 34, projected_percent: 79, spark: '▁▂▃▄',
            spark_points: [12, 13, 14]}],
    }, now);
    assert.equal(paced.rows[0].projected, 79);
    assert.equal(paced.rows[0].spark, '▁▂▃▄');
    assert.deepEqual(paced.rows[0].sparkPoints, [12, 13, 14]);
    const missing = projectEntry({
        id: 'paced',
        sections: [{type: 'metric', label: 'Weekly (7d)', percent: 27, value: '27%',
            headline: 'percent', severity: 'low', detail: 'Resets in 3d'}],
    }, now);
    assert.equal(missing.rows[0].projected, null);
    assert.equal(missing.rows[0].spark, '');
    assert.deepEqual(missing.rows[0].sparkPoints, []);
}
assert.equal(claude.rows[0].valueText, '84%');
assert.equal(claude.rows[0].severity, 'high');
assert.equal(claude.rows[0].reset, '1h 7m');
assert.equal(claude.rows[0].elapsed, 77);
assert.equal(claude.rows[0].detail, '77% elapsed · 7pts ahead');
// A real 0% window is a row, not an absent one.
assert.equal(claude.rows[1].valueText, '0%');

// Antigravity's pool headings arrive as text sections, not as `group`.
const antigravity = projectEntry({
    id: 'antigravity',
    display_name: 'Antigravity',
    plan: 'Google AI Pro',
    sections: [
        {type: 'spacer'},
        {type: 'text', label: 'Session', value: ''},
        {type: 'metric', label: 'Gemini', percent: 0, detail: 'Resets in 4h 52m',
            reset_at: '2026-09-24T22:52:00Z'},
        {type: 'metric', label: 'Claude & GPT OSS', percent: 0, detail: 'Resets in 4h 52m',
            reset_at: '2026-09-24T22:52:00Z'},
        {type: 'text', label: 'Weekly', value: ''},
        {type: 'metric', label: 'Gemini', percent: 25, detail: 'Resets in 1d 20h',
            reset_at: '2026-09-26T14:00:00Z'},
    ],
}, now);
assert.deepEqual(labels(antigravity.rows),
    ['text:Session', 'metric:Gemini', 'metric:Claude & GPT OSS', 'text:Weekly', 'metric:Gemini']);
assert.equal(antigravity.rows[1].detail, '');
assert.equal(antigravity.rows[1].elapsed, null);

const cursor = projectEntry({
    id: 'cursor',
    display_name: 'Cursor',
    plan: 'Cursor Enterprise',
    sections: [
        {type: 'metric', label: 'Cursor Models', percent: 44, detail: 'Auto + Composer',
            reset_at: '2026-10-22T01:58:45Z'},
        {type: 'text', label: 'On-Demand', value: '$0.00'},
    ],
}, now);
assert.equal(cursor.rows[0].detail, 'Auto + Composer');
assert.deepEqual(cursor.rows[1], {type: 'text', label: 'On-Demand', value: '$0.00'});

const codex = projectEntry({
    id: 'openai',
    display_name: 'Codex',
    sections: [
        {type: 'block', label: 'Credits', body: ['balance: 0', '', '≈ 0-0 local messages']},
        {type: 'block', label: '', body: []},
    ],
}, now);
assert.deepEqual(codex.rows, [{type: 'block', label: 'Credits', body: ['balance: 0', '≈ 0-0 local messages']}]);

const balance = projectEntry({
    id: 'deepseek',
    display_name: 'DeepSeek',
    sections: [{type: 'metric', label: 'Balance', percent: 40, headline: 'value', value: '$12.50'}],
}, now);
assert.equal(balance.rows[0].valueText, '$12.50');
assert.equal(balance.rows[0].headline, 'value');
// A value headline with nothing to show falls back to the percent.
assert.equal(projectEntry({id: 'd', sections: [
    {type: 'metric', label: 'Balance', percent: 40, headline: 'value', value: ''}]}, now).rows[0].valueText, '40%');

// A window with no percent is left out, not drawn as 0%.
const absent = projectEntry({
    id: 'openai',
    sections: [
        {type: 'metric', label: 'Session', percent: null},
        {type: 'metric', label: 'Weekly', percent: 30, severity: 'mid'},
    ],
}, now);
assert.deepEqual(labels(absent.rows), ['metric:Weekly']);
assert.equal(absent.rows[0].severity, 'mid');

const failed = projectEntry({
    id: 'cursor',
    display_name: 'Cursor',
    plan: 'Ultra',
    error: 'credentials error:\nsign in once',
    sections: [{type: 'metric', label: 'Cursor Models', percent: 0}],
}, now);
assert.equal(failed.error, 'credentials error: sign in once');
assert.deepEqual(failed.rows, []);

// SuperGrok's `group`: a heading over consecutive grouped metrics. An
// ungrouped row closes it, and the group's next metric gets it again.
const grouped = projectEntry({
    id: 'supergrok',
    sections: [
        {type: 'metric', label: 'Overall', percent: 40},
        {type: 'metric', label: 'Chat', percent: 10, group: 'Breakdown'},
        {type: 'metric', label: 'Imagine', percent: 20, group: 'Breakdown'},
        {type: 'text', label: 'Plan', value: 'Heavy'},
        {type: 'metric', label: 'Voice', percent: 5, group: 'Breakdown'},
    ],
}, now);
assert.deepEqual(labels(grouped.rows), ['metric:Overall', 'group:Breakdown', 'metric:Chat',
    'metric:Imagine', 'text:Plan', 'group:Breakdown', 'metric:Voice']);

// A section kind a newer binary adds degrades to a readable line.
assert.deepEqual(projectEntry({id: 'x', sections: [{type: 'gauge', label: 'Future', value: '7'}]}, now).rows,
    [{type: 'text', label: 'Future', value: '7'}]);

const report = parseReport(JSON.stringify({
    schema_version: 1,
    entries: [
        {id: 'anthropic', display_name: 'Claude', sections: [{type: 'metric', label: 'Weekly', percent: 83}]},
        {id: '', display_name: 'dropped'},
    ],
}), now);
assert.equal(report.ok, true);
assert.equal(report.entries.length, 1);
assert.equal(report.entries[0].rows[0].severity, 'high');

assert.equal(parseReport('not json').ok, false);
assert.equal(parseReport('null').ok, false);
assert.equal(parseReport('[]').ok, false);
assert.equal(parseReport('{"entries":[{"name":1}]}').ok, false);
assert.deepEqual(parseReport('{"entries":[]}'), {ok: true, error: '', entries: []});

// `usage` with nothing enabled exits 1 with only stderr.
assert.equal(commandFailure('ai-usagebar usage: no vendors enabled in ~/.config/ai-usagebar/config.toml\n').error,
    'ai-usagebar usage: no vendors enabled in ~/.config/ai-usagebar/config.toml');
assert.equal(commandFailure('').ok, false);
assert.notEqual(commandFailure('').error, '');

assert.equal(errorLine('ai-usagebar took too long', 'timed out after 60s'),
    'ai-usagebar took too long: timed out after 60s');
assert.equal(errorLine('invalid output', ''), 'invalid output');
assert.equal(errorLine('invalid output', 'x'.repeat(1000)).length, 'invalid output: '.length + 300);
assert.ok(!errorLine('failed', 'a\nb').includes('\n'));

// Daily and three-hour limits retain the report's labels and order; they
// must not be relabelled as five-hour or weekly windows.
const custom = projectEntry({id: 'custom:quota', display_name: 'Team quota', sections: [
    {type: 'metric', label: 'Daily (1d)', percent: 20, window_secs: 86400},
    {type: 'metric', label: 'Burst (3h)', percent: 70, window_secs: 10800},
]}, now);
assert.deepEqual(labels(custom.rows), ['metric:Daily (1d)', 'metric:Burst (3h)']);
assert.equal(custom.title, 'Team quota');
assert.equal(projectEntry({id: 'openai@work', display_name: 'Codex (work)', stale: true}, now).stale, true);
assert.equal(metricDetail('Resets in 3d', 'invalid'), 'Resets in 3d');
assert.equal(projectEntry({id: 'balance', sections: [
    {type: 'metric', label: 'Balance', percent: 0, headline: 'value', value: '-$1.25'},
]}, now).rows[0].valueText, '-$1.25');
// Named accounts and custom entries need no frontend provider registration.
const many = Array.from({length: 70}, (_, i) => ({id: `custom:account-${i}`, sections: []}));
assert.equal(parseReport(JSON.stringify({entries: many}), now).entries.length, 70);
assert.deepEqual(projectEntry({id: 'anthropic', sections: [
    {type: 'block', label: 'Banked resets', body: ['1 available', 'Expires in 2d']},
]}, now).rows, [{type: 'block', label: 'Banked resets', body: ['1 available', 'Expires in 2d']}]);

const synthetic = projectEntry({
    id: 'synthetic',
    display_name: 'Synthetic',
    plan: 'Standard',
    sections: [
        {type: 'metric', label: 'Weekly credits', percent: 30, value: '30%',
            detail: 'Regenerates 2% in 3h 04m', regen: '2%', reset_at: '2026-09-24T21:04:00Z'},
    ],
}, now);
assert.equal(synthetic.rows[0].regen, '2%');
assert.equal(synthetic.rows[0].detail, '');
assert.equal(synthetic.rows[0].reset, '3h 4m');

// Overview previews preserve order, exact labels and values, including zero.
const preview = entry => summarize(entry.rows);
assert.deepEqual(preview(claude).rows.map(row => [row.label, row.valueText]),
    [['Session (5h)', '84%'], ['Weekly (7d)', '0%']]);
assert.equal(preview(claude).remaining, 0);
assert.deepEqual(preview(custom).rows.map(row => row.label), ['Daily (1d)', 'Burst (3h)']);
assert.deepEqual(preview(antigravity).rows.map(row => row.label),
    ['Session · Gemini', 'Session · Claude & GPT OSS']);
assert.equal(preview(antigravity).remaining, 1);
assert.deepEqual(preview(grouped).rows.map(row => row.label), ['Overall', 'Breakdown · Chat']);
assert.equal(preview(grouped).remaining, 2);
assert.deepEqual(preview(projectEntry({id: 'x', sections: [
    {type: 'metric', label: 'Chat', group: 'Breakdown', percent: 10},
    {type: 'metric', label: 'Overall', percent: 20},
]})).rows.map(row => row.label), ['Breakdown · Chat', 'Overall']);
assert.deepEqual(preview(failed), {rows: [], remaining: 0});
assert.equal(preview(balance).rows[0].headline, 'value');
assert.equal(preview(balance).rows[0].valueText, '$12.50');
assert.deepEqual(preview(projectEntry({id: 'x', sections: [
    {type: 'text', label: 'Balance', value: '-$1.25'},
]})), {rows: [{label: 'Balance', valueText: '-$1.25', headline: 'value'}], remaining: 0});
assert.deepEqual(preview(absent).rows.map(row => row.label), ['Weekly']);

// Future providers need no registration. Unsafe brand paths never name assets.
assert.equal(brandSlug('new-brand', 'custom:future'), 'new-brand');
assert.equal(brandSlug(undefined, 'anthropic@work'), 'anthropic');
assert.equal(brandSlug('', 'custom:future'), '');
assert.equal(brandSlug('../../etc/passwd', 'anthropic'), '');
assert.equal(brandSlug('UPPERCASE', 'x'), '');

console.log('report model tests passed');
