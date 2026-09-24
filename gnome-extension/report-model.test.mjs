import assert from 'node:assert/strict';
import {finitePercent, formatReset, parseReport, projectEntry} from './report-model.js';

const now = Date.parse('2026-09-24T18:00:00Z');

assert.equal(finitePercent(37), 37);
assert.equal(finitePercent('83'), 83);
assert.equal(finitePercent(null), null);
assert.equal(finitePercent(undefined), null);
assert.equal(finitePercent(''), null);
assert.equal(finitePercent(-1), null);
assert.equal(finitePercent(101), null);

assert.equal(formatReset('2026-09-24T21:35:00Z', now), '3h 35m');
assert.equal(formatReset('2026-09-24T17:00:00Z', now), 'now');
assert.equal(formatReset('', now), '');
assert.equal(formatReset('not-a-date', now), '');

const percent = projectEntry({
    id: 'anthropic',
    display_name: 'Claude',
    plan: 'Pro',
    sections: [{
        type: 'metric',
        label: 'Session',
        percent: 37,
        headline: 'percent',
        reset_at: '2026-09-24T21:35:00Z',
    }],
}, now);
assert.equal(percent.title, 'Claude');
assert.equal(percent.plan, 'Pro');
assert.equal(percent.rows.length, 1);
assert.equal(percent.rows[0].valueText, '37%');
assert.equal(percent.rows[0].reset, '3h 35m');
assert.equal(percent.rows[0].severity, 'low');

const balance = projectEntry({
    id: 'deepseek',
    display_name: 'DeepSeek',
    sections: [{
        type: 'metric',
        label: 'Balance',
        percent: 40,
        headline: 'value',
        value: '$12.50',
    }],
}, now);
assert.equal(balance.rows[0].valueText, '$12.50');
assert.equal(balance.rows[0].headline, 'value');

const absent = projectEntry({
    id: 'openai',
    display_name: 'Codex',
    sections: [
        {type: 'metric', label: 'Session', percent: null},
        {type: 'metric', label: 'Weekly', percent: 30, severity: 'mid'},
    ],
}, now);
assert.deepEqual(absent.rows.map(row => row.label), ['Weekly']);
assert.equal(absent.rows[0].percent, 30);
assert.notEqual(absent.rows[0].valueText, '0%');

const failed = projectEntry({
    id: 'cursor',
    display_name: 'Cursor',
    plan: 'Ultra',
    error: 'sign in once',
    sections: [{type: 'metric', label: 'Cursor Models', percent: 0}],
}, now);
assert.equal(failed.error, 'sign in once');
assert.deepEqual(failed.rows, []);

const grouped = projectEntry({
    id: 'antigravity',
    display_name: 'Antigravity',
    sections: [
        {type: 'metric', label: 'Gemini', percent: 18, group: 'Session'},
        {type: 'metric', label: 'Claude & GPT', percent: 54, group: 'Session'},
        {type: 'metric', label: 'Gemini', percent: 11, group: 'Weekly'},
    ],
}, now);
assert.deepEqual(grouped.rows.map(row => row.type === 'group' ? row.label : row.label),
    ['Session', 'Gemini', 'Claude & GPT', 'Weekly', 'Gemini']);

const text = projectEntry({
    id: 'openrouter',
    display_name: 'OpenRouter',
    sections: [{type: 'text', label: 'Balance', value: '$13.67'}],
}, now);
assert.deepEqual(text.rows[0], {type: 'text', label: 'Balance', value: '$13.67'});

const report = parseReport(JSON.stringify({
    schema_version: 1,
    entries: [
        {id: 'anthropic', display_name: 'Claude', sections: [{type: 'metric', label: 'Weekly', percent: 83}]},
        {id: '', display_name: 'dropped'},
    ],
}), now);
assert.equal(report.ok, true);
assert.equal(report.entries.length, 1);
assert.equal(report.entries[0].rows[0].valueText, '83%');
assert.equal(report.entries[0].rows[0].severity, 'high');

assert.equal(parseReport('not json').ok, false);
assert.equal(parseReport('{"entries":[{"name":1}]}').ok, false);
assert.equal(parseReport('{"entries":[]}').ok, true);
