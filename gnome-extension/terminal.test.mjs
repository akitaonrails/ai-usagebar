import assert from 'node:assert/strict';
import fs from 'node:fs';

import {terminalCandidates} from './terminal.js';

const command = 'export PATH="$HOME/.local/bin:$PATH" && \'ai-usagebar-tui\'';
assert.deepEqual(terminalCandidates(command), [
  ['ptyxis', '--', 'bash', '-lc', command],
  ['kgx', '--', 'bash', '-lc', command],
  ['gnome-terminal', '--', 'bash', '-lc', command],
  ['xterm', '-e', 'bash', '-lc', command],
]);

const extension = fs.readFileSync(new URL('./extension.js', import.meta.url), 'utf8');
const prefs = fs.readFileSync(new URL('./prefs.js', import.meta.url), 'utf8');
for (const source of [extension, prefs]) {
  assert.match(source, /terminalCandidates\(line\)/);
  assert.match(source, /export PATH="\$HOME\/\.local\/bin:\$PATH"/);
}
assert.match(extension, /GLib\.shell_quote\(tui\)/);
assert.match(prefs, /commandArgv\.map\(a => GLib\.shell_quote\(a\)\)/);

console.log('GNOME terminal launcher tests passed');
