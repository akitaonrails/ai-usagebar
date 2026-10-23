import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const priority = vm.createContext({});
vm.runInContext(fs.readFileSync(new URL('./Priority.js', import.meta.url), 'utf8'), priority);
const i18n = vm.createContext({});
vm.runInContext(fs.readFileSync(new URL('./I18n.js', import.meta.url), 'utf8'), i18n);

for (const [input, expected] of [[undefined,3],[null,3],['',3],['bad',3],['6',6],[0,1],[25,24],[2.7,3]])
  assert.equal(priority.normalizeHours(input), expected);

for (const [input, expected] of [[undefined,50],['',50],['x',50],[0,1],[150,100],['30',30]])
  assert.equal(priority.normalizeAlertPercent(input), expected);
for (const [input, expected] of [[undefined,24],[0,1],[500,168],['48',48]])
  assert.equal(priority.normalizeAlertHours(input), expected);
assert.deepEqual(JSON.parse(JSON.stringify(priority.choices([10, 50], 33, v => v + '%'))),
  [{value:'10',label:'10%'},{value:'33',label:'33%'},{value:'50',label:'50%'}], 'A value set outside the form stays selectable');

const now = Date.parse('2026-10-21T12:00:00Z');
const row = {status:'priority', reason_code:'priority', urgent:true, window:'Weekly',
  reset_at:'2026-10-21T18:00:00Z', unused_percent:40, remaining_percent:69, cycle_percent_per_hour:1.5,
  recent_percent_per_hour:0, recent_hours:2, pace_percent_per_hour:0.5, required_percent_per_hour:7,
  alert_unused_percent:50, alert_hours:24};
const tr = (key, params) => i18n.t('pt-BR',key,params);
assert.equal(priority.stateKey(row), 'urgent');
assert.equal(priority.stateKey({...row,status:'on_track'}), 'on_track');
assert.equal(priority.stateKey({status:'blocked'}), 'blocked');
assert.equal(priority.reasonKey({reason_code:'not_a_status'}), 'unknown');
const plain = v => JSON.parse(JSON.stringify(v));
// Facts: one value per row, locale decimal separator, no duplicated summary.
assert.deepEqual(plain(priority.facts({...row, pace_percent_per_hour:1.25, recent_hours:1.5}, now, true, tr)), [
  {label:'Janela', value:'Weekly · renova em 6h'},
  {label:'Restante agora', value:'69%'},
  {label:'Sobra prevista', value:'40%', strong:true},
  {label:'Seu ritmo', value:'1,3%/h'},
  {label:'Últimas 1,5h', value:'0%/h'},
  {label:'Média do ciclo', value:'1,5%/h'},
  {label:'Para usar tudo', value:'7%/h'},
]);
// Without enough history the recent row says why instead of showing a rate.
assert.deepEqual(plain(priority.facts({...row, recent_percent_per_hour:null}, now, true, tr)[4]), {label:'Ritmo recente', value:'coletando (mín. 30 min)'});
assert.equal(priority.facts({...row, recent_percent_per_hour:null}, now, false, tr)[4].value, 'indisponível');
// Notes: the conclusion first, then urgency, session and cache qualifiers.
assert.deepEqual(plain(priority.notes({...row, session_exhausted:true, stale:true, fetched_at:'2026-10-21T09:00:00Z'}, now, tr)), [
  'No ritmo atual, vai sobrar cota na renovação. Use esta IA antes das outras.',
  'Urgente: renova em até 24h com pelo menos 50% de sobra prevista.',
  'O limite da sessão acabou por agora. Ele volta em poucas horas e não muda a previsão semanal.',
  'Dados do cache, de 3h atrás.',
]);
assert.equal(priority.summary(row, now, tr), 'Sobra prevista: 40% de 69% · renova em 6h');
assert.equal(priority.summary({...row, reset_at:'2026-10-25T15:00:00Z'}, now, tr), 'Sobra prevista: 40% de 69% · renova em 4d 3h');
assert.equal(priority.summary({...row, unused_percent:44.4, remaining_percent:69.2}, now, (k,p) => i18n.t('en',k,p)), 'Projected unused: 44.4% of 69.2% · resets in 6h');
// A row without a forecast shows only its reason — never "undefined%".
assert.equal(priority.summary({status:'blocked', window:'Weekly', reset_at:'2026-10-22T00:00:00Z'}, now, tr), 'A cota semanal acabou; aguarde a renovação.');
assert.deepEqual(plain(priority.facts({status:'unavailable'}, now, true, tr)), []);
assert.equal(priority.detail({status:'unavailable'},now,true,tr), 'Sem dados de uso atualizados.');
assert.equal(priority.detail(null,now,true,tr),'');
assert.match(priority.detail(row, now, true, tr), /^Janela: Weekly · renova em 6h\n/);
for (const locale of i18n.SUPPORTED) {
  const localized = priority.detail(row,now,true,(k,p) => i18n.t(locale,k,p));
  assert.doesNotMatch(localized,/priority\.|\{\w+\}|undefined|NaN/,`${locale} has complete rationale messages`);
  assert.doesNotMatch(priority.summary(row,now,(k,p) => i18n.t(locale,k,p)),/priority\.|\{\w+\}/,`${locale} summary`);
}
// Generic window labels follow the locale; provider-specific ones and English stay as sent.
assert.equal(i18n.displayLabel('pt-BR', 'Weekly (7d)'), 'Semanal (7d)');
assert.equal(i18n.displayLabel('pt-BR', 'Session (5h)'), 'Sessão (5h)');
assert.equal(i18n.displayLabel('ru', 'Weekly'), 'Неделя (7 дн.)');
assert.equal(i18n.displayLabel('pt-BR', 'Codex weekly'), 'Codex weekly');
assert.equal(i18n.displayLabel('en', 'Weekly'), 'Weekly');
// Copy we added uses no dashes or curly quotes in any locale.
{
  const src = fs.readFileSync(new URL('./I18n.js', import.meta.url), 'utf8');
  const ours = src.split('\n').filter(l => /"(priority|notice|toggle\.tab_names|view\.usage)[^"]*":/.test(l));
  assert.ok(ours.length > 300);
  for (const line of ours) assert.doesNotMatch(line, /[—–“”]/, line.trim());
}
console.log('Priority presentation tests passed');
