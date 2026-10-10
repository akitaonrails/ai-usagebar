import { expect, mock, test } from 'claude-code/testing'
import type { On, ProcessRunResult } from 'claude-code'
import type { Engine } from 'claude-code/testing'

import { JOB_PREFIX, JOB_SCRIPT } from '../hooks/job'

const BIN = '/home/u/.cargo/bin/ai-usagebar'
const LOG = '/home/u/Library/Logs/ai-usagebar-desktop-switch.log'
const SURFACES = ['terminal', 'desktop'] as const

const STATUS = {
  desktop: {
    available: true,
    active_label: 'hotmail',
    profiles: [
      { label: 'gmail', email: 'a@gmail.com', active: false, has_credentials: true },
      { label: 'hotmail', email: 'b@hotmail.com', active: true, has_credentials: true },
    ],
  },
  cli: {},
}

const USAGE = {
  entries: [
    {
      id: 'anthropic@gmail',
      sections: [
        { type: 'metric', label: 'Session (5h)', percent: 2, resets_at: null },
        { type: 'metric', label: 'Weekly (7d)', percent: 93, resets_at: null },
        { type: 'note', label: 'plan' },
      ],
    },
    { id: 'openai@gmail', sections: [{ type: 'metric', label: 'Weekly', percent: 50 }] },
  ],
}

const result = (exitCode: number, stdout: string, stderr = ''): { value: ProcessRunResult } => ({
  value: { exitCode, stdout, stderr, isStdoutTruncated: false, isStderrTruncated: false },
})

type World = { argvs: string[][]; statuses: (string | undefined)[]; toasts: string[]; status: object; dryRunExit: number }

function world(on: On, files = [BIN]): World {
  const w: World = { argvs: [], statuses: [], toasts: [], status: STATUS, dryRunExit: 0 }
  mock.env(on, { HOME: '/home/u', PATH: '/usr/bin:/bin' })
  on('fs.exists', ($, e) => ({ value: files.includes(e.path) }))
  on('session.start', ($, e) => ({ cwd: e.cwd }) as never)
  on('command.register', () => ({ value: {} }) as never)
  on('ui.open', () => ({ value: { isPlaced: true } }) as never)
  on('ui.status', ($, e) => (w.statuses.push(e.text), { value: undefined }))
  on('ui.toast', ($, e) => (w.toasts.push(e.text), { value: undefined }))
  on('process.run', ($, e) => {
    const argv = [...e.argv]
    w.argvs.push(argv)
    if (argv[0] === '/bin/launchctl') return result(0, '')
    if (argv.includes('status')) return result(0, JSON.stringify(w.status))
    if (argv.includes('usage')) return result(0, JSON.stringify(USAGE))
    if (argv.includes('--dry-run')) return result(w.dryRunExit, 'Claude Desktop   → gmail\n  (dry run — nothing was changed)')
    return result(1, '', 'unexpected')
  })
  return w
}

const start = ($: { session: { start: (e: { cwd: string; surface: null; isInteractive: boolean }) => Promise<unknown> } }) =>
  $.session.start({ cwd: '/tmp', surface: null, isInteractive: true })

const PANE = { title: 'Claude accounts', isFocused: true, bodyColumns: 80, placement: 'dock' } as never
const open = async ($: Engine, surface: (typeof SURFACES)[number]) => {
  await $.command.run({ command: 'claude-account', args: '', origin: { kind: 'composer' } } as never)
  return $.ui.mount({ plugin: 'desktop-accounts', surface, component: 'Pane', requestId: 'claude-account', props: PANE })
}

// No call may run a real switch: every `account switch` is a dry run or goes through launchctl.
const realSwitches = (w: World) =>
  w.argvs.filter(a => a[0] !== '/bin/launchctl' && a.includes('switch') && !a.includes('--dry-run'))

test('lists the Desktop accounts with the active one marked', async ($, on) => {
  const w = world(on)
  await start($)
  for (const surface of SURFACES) {
    const pane = await open($, surface)
    expect(await pane.find({ type: 'Text', text: 'gmail' })).toBeDefined()
    expect(await pane.find({ type: 'Text', text: 'a@gmail.com' })).toBeDefined()
    expect(await pane.find({ type: 'Text', text: 'active' })).toBeDefined()
    expect(await pane.find({ key: 'switch:gmail' })).toBeDefined()
    expect(await pane.find({ key: 'switch:hotmail' })).toBeUndefined()
    expect(await pane.find({ type: 'Text', text: 'Switching quits and reopens the app.' })).toBeDefined()
    await pane.unmount()
  }
  expect(w.argvs[0]).toEqual([BIN, 'account', 'status', '--json'])
  expect(w.statuses.at(-1)).toBe('Desktop: hotmail')
})

test('says so when there is no Claude Desktop app', async ($, on) => {
  const w = world(on)
  w.status = { desktop: null }
  await start($)
  const pane = await open($, 'desktop')
  expect(await pane.find({ type: 'Text', text: /No Claude Desktop app/ })).toBeDefined()
  expect(await pane.find({ key: 'switch:gmail' })).toBeUndefined()
  await pane.unmount()
})

test('Switch asks first, shows the dry-run plan, and Cancel launches nothing', async ($, on) => {
  const w = world(on)
  await start($)
  for (const surface of SURFACES) {
    const pane = await open($, surface)
    await pane.press({ key: 'switch:gmail' })
    expect(w.argvs.at(-1)).toEqual([BIN, 'account', 'switch', '--desktop', '--dry-run', '--', 'gmail'])
    expect(await pane.find({ type: 'Text', text: 'Switch Claude Desktop to gmail?' })).toBeDefined()
    expect(await pane.find({ type: 'Text', text: /→ gmail[\s\S]*dry run/ })).toBeDefined()
    expect(await pane.find({ key: 'go' })).toBeDefined()
    await pane.press({ key: 'cancel' })
    expect(await pane.find({ type: 'Text', text: /^Switch Claude Desktop to/ })).toBeUndefined()
    expect(await pane.find({ key: 'switch:gmail' })).toBeDefined()
    await pane.unmount()
  }
  expect(w.argvs.some(a => a[0] === '/bin/launchctl')).toBe(false)
  expect(realSwitches(w)).toEqual([])
})

test('a failed dry run offers no way to switch', async ($, on) => {
  const w = world(on)
  w.dryRunExit = 1
  await start($)
  const pane = await open($, 'desktop')
  await pane.press({ key: 'switch:gmail' })
  expect(await pane.find({ key: 'go' })).toBeUndefined()
  expect(await pane.find({ key: 'cancel' })).toBeDefined()
  await pane.unmount()
})

test('confirming launches the switch as a detached launchd job with the label as its own argument', async ($, on) => {
  const w = world(on)
  await start($)
  const pane = await open($, 'desktop')
  await pane.press({ key: 'switch:gmail' })
  await pane.press({ key: 'go' })
  const submit = w.argvs.find(a => a[0] === '/bin/launchctl')!
  const job = submit[3]!
  expect(job.startsWith(`${JOB_PREFIX}.`)).toBe(true)
  expect(submit).toEqual([
    '/bin/launchctl', 'submit', '-l', job, '-o', LOG, '-e', LOG, '--',
    '/bin/sh', '-c', JOB_SCRIPT, 'sh', job, BIN, 'account', 'switch', '--desktop', '--yes', '--', 'gmail',
  ])
  expect(realSwitches(w)).toEqual([])
  expect(await pane.find({ type: 'Text', text: /^Switching Claude Desktop to gmail/ })).toBeDefined()
  expect(w.toasts.at(-1)).toContain('gmail')
  await pane.unmount()
})

test('falls back to the AI Usage app bundle tray binary', async ($, on) => {
  const tray = '/Applications/AI Usage.app/Contents/MacOS/ai-usagebar-tray'
  const w = world(on, [tray])
  await start($)
  const pane = await open($, 'terminal')
  expect(w.argvs[0]).toEqual([tray, 'account', 'status', '--json'])
  await pane.unmount()
})

test('each account shows its quota windows, colored by how much is used', async ($, on) => {
  world(on)
  await start($)
  for (const surface of SURFACES) {
    const ui = await open($, surface)
    const texts = (await ui.findAll({ type: 'Text' })).map(t => t.text)
    expect(texts).toContain('5h')
    expect(texts).toContain('7d')
    expect(texts).toContain('93%')
    expect((await ui.find({ type: 'Text', text: '▰▰▰▰▰▰▰▱' }))?.props.color).toBe('red')
    expect(texts).toContain('  no quota reading')
    expect(texts.some(t => t === '50%')).toBe(false)
    await ui.unmount()
  }
})
