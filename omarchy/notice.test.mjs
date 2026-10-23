import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const notice = vm.createContext({});
vm.runInContext(fs.readFileSync(new URL('./Notice.js', import.meta.url), 'utf8'), notice);
const i18n = vm.createContext({});
vm.runInContext(fs.readFileSync(new URL('./I18n.js', import.meta.url), 'utf8'), i18n);
const plain = value => JSON.parse(JSON.stringify(value));

// The real OpenCode Go message: prefix dropped, command lifted out.
assert.deepEqual(plain(notice.build('credentials error: OpenCode Go is not signed in. Run `opencode auth login` and choose OpenCode, or set OPENCODE_GO_API_KEY.', 'error')), {
  kind: 'error', category: 'credentials',
  body: 'OpenCode Go is not signed in. Run "opencode auth login" and choose OpenCode, or set OPENCODE_GO_API_KEY.',
  command: 'opencode auth login', retry: false });
// The missing-binary message names its install command without backticks.
assert.equal(notice.build('ai-usagebar is not installed. The plugin is only the display frontend. Install the binary with: omarchy pkg aur add ai-usagebar-bin', 'error', 'missing').command,
  'omarchy pkg aur add ai-usagebar-bin');
for (const [text, category, retry] of [
  ['HTTP error: 503 Service Unavailable', 'network', true],
  ['request timed out after 10s', 'network', true],
  ['rate limited; next attempt in 4m', 'rate_limit', true],
  ['token expired, run `claude` to re-auth', 'credentials', false],
  ['unexpected payload', 'generic', false],
]) {
  const n = notice.build(text, 'error');
  assert.equal(n.category, category, text);
  assert.equal(n.retry, retry, text);
}
// Info notices never offer a retry; empty text is no notice at all.
assert.equal(notice.build('Cached data', 'info', 'cached').retry, false);
assert.equal(notice.build('  ', 'error'), null);
// Only a plain command is offered for copying, never arbitrary prose.
assert.equal(notice.commandOf('see `the docs page` for help'), 'the docs page');
assert.equal(notice.commandOf('use `$(rm -rf ~)` now'), '');
assert.equal(notice.commandOf('no command here'), '');
for (const locale of i18n.SUPPORTED)
  for (const key of ['credentials','network','rate_limit','refresh','filter','cached','missing','generic','copy','copied','retry'])
    assert.notEqual(i18n.t(locale, 'notice.' + key), 'notice.' + key, `${locale} notice.${key}`);
for (const locale of i18n.SUPPORTED)
  for (const key of ['credentials','network','rate_limit','refresh','filter','cached','missing','generic'])
    assert.notEqual(i18n.t(locale, 'notice.short.' + key), 'notice.short.' + key, `${locale} notice.short.${key}`);
assert.equal(notice.glyph(notice.build('credentials error: x', 'error')), '󰌾');
assert.equal(notice.glyph(notice.build('Cached', 'info', 'cached')), '󰋼');
console.log('Notice presentation tests passed');
