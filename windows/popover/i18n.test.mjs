import assert from 'node:assert/strict';
import { readdir, readFile, mkdtemp, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { compile } from '@inlang/paraglide-js';

// Paths from this file, so the test runs from the repo root as well as from windows/popover.
const here = path.dirname(fileURLToPath(import.meta.url));
process.chdir(here); // the inlang project loads its plugin from ./node_modules
const project = path.join(here, 'project.inlang');
const checkedIn = path.join(here, 'src/paraglide');
const temporary = await mkdtemp(path.join(os.tmpdir(), 'ai-usagebar-paraglide-'));
const compiled = path.join(temporary, 'paraglide');
const english = JSON.parse(await readFile(path.join(here, 'messages/en.json'), 'utf8'));
const portuguese = JSON.parse(await readFile(path.join(here, 'messages/pt-BR.json'), 'utf8'));
// The compiler's README records the absolute project path of whoever compiled it, so it differs
// on every machine; `npm run i18n` deletes it and the comparison leaves it out.
const MACHINE_SPECIFIC = new Set(['README.md']);

assert.deepEqual(Object.keys(portuguese).sort(), Object.keys(english).sort(), 'English and Portuguese message catalogs must have matching keys.');
assert.ok(Object.values(english).every((value) => typeof value === 'string' && value.trim()), 'English catalog messages must not be empty.');
assert.ok(Object.values(portuguese).every((value) => typeof value === 'string' && value.trim()), 'Portuguese catalog messages must not be empty.');

async function filesUnder(directory) {
  const files = new Map();
  async function visit(current) {
    for (const entry of await readdir(current, { withFileTypes: true })) {
      const absolute = path.join(current, entry.name);
      if (entry.isDirectory()) {
        await visit(absolute);
      } else if (entry.isFile() && !MACHINE_SPECIFIC.has(path.relative(directory, absolute))) {
        files.set(path.relative(directory, absolute), await readFile(absolute, 'utf8'));
      }
    }
  }
  await visit(directory);
  return files;
}

try {
  await compile({
    project,
    outdir: compiled,
    strategy: ['globalVariable', 'baseLocale'],
    outputStructure: 'locale-modules',
    emitGitIgnore: false,
  });

  const expected = await filesUnder(compiled);
  const actual = await filesUnder(checkedIn);
  assert.deepEqual([...actual.keys()].sort(), [...expected.keys()].sort(), 'Generated Paraglide files are stale. Run npm run i18n.');
  for (const [file, contents] of expected) {
    assert.equal(actual.get(file), contents, `Generated Paraglide output is stale: ${file}. Run npm run i18n.`);
  }
  console.log('Paraglide generated output: fresh');
} finally {
  await rm(temporary, { recursive: true, force: true });
}
