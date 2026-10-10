import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

import type { DesktopStatus, QuotaMetric } from '../types'
import { JOB_PREFIX, JOB_SCRIPT, LOG } from './job'

type Engine = EngineInterface

const PANE = 'claude-account'
const COMMAND = 'claude-account'
const BINARIES = ['ai-usagebar', 'ai-usagebar-tray']
const APP_DIRS = ['/Applications/AI Usage.app/Contents/MacOS', '~/Applications/AI Usage.app/Contents/MacOS']
const FIXED_DIRS = ['~/.cargo/bin', '/opt/homebrew/bin', '/usr/local/bin']

const desktop = atom({ plugin: 'desktop-accounts', key: 'desktop' } as const, null)
const problem = atom({ plugin: 'desktop-accounts', key: 'problem' } as const, '')
const pending = atom({ plugin: 'desktop-accounts', key: 'pending' } as const, null)
const launched = atom({ plugin: 'desktop-accounts', key: 'launched' } as const, '')
const quota = atom({ plugin: 'desktop-accounts', key: 'quota' } as const, null)

let bin: string | undefined

async function home($: Engine): Promise<string> {
  return (await $.env.get('HOME')) ?? ''
}

async function resolveBin($: Engine): Promise<string | undefined> {
  if (bin) return bin
  const h = await home($)
  const path = ((await $.env.get('PATH')) ?? '').split(':').filter(Boolean)
  const dirs = [...path, ...FIXED_DIRS, ...APP_DIRS].map(d => d.replace(/^~(?=\/)/, h))
  for (const name of BINARIES) {
    for (const dir of dirs) {
      if (await $.fs.exists(`${dir}/${name}`)) return (bin = `${dir}/${name}`)
    }
  }
  return undefined
}

async function cli($: Engine, argv: string[]) {
  const b = await resolveBin($)
  if (!b) throw new Error('ai-usagebar not found (PATH, ~/.cargo/bin, /Applications/AI Usage.app)')
  return $.process.run([b, ...argv], { timeoutMs: 60_000 })
}

async function refresh($: Engine): Promise<DesktopStatus | null> {
  try {
    const out = await cli($, ['account', 'status', '--json'])
    if (out.exitCode !== 0) throw new Error(out.stderr.trim() || `exit ${out.exitCode}`)
    const raw = JSON.parse(out.stdout).desktop as {
      active_label: string | null
      profiles: { label: string; email: string | null; active: boolean }[]
    } | null
    const status: DesktopStatus | null = raw && {
      active_label: raw.active_label,
      profiles: raw.profiles.map(p => ({ label: p.label, email: p.email ?? null, active: p.active })),
    }
    await update($, desktop, () => status)
    await update($, problem, () => '')
    $.ui.status(status?.active_label ? `Desktop: ${status.active_label}` : undefined)
    return status
  } catch (err) {
    await update($, problem, () => `account status failed: ${err instanceof Error ? err.message : err}`)
    return null
  }
}

// ponytail: one `usage --json` for every vendor; a per-account flag would spare the other fetches.
async function loadQuota($: Engine): Promise<void> {
  try {
    const out = await cli($, ['usage', '--json'])
    if (out.exitCode !== 0) return
    const parsed = JSON.parse(out.stdout) as { entries?: unknown } | unknown[]
    const entries = (Array.isArray(parsed) ? parsed : (parsed.entries as unknown[]) ?? []) as {
      id?: string
      sections?: { type?: string; label?: string; percent?: number; resets_at?: string | null }[]
    }[]
    const byLabel: Record<string, QuotaMetric[]> = {}
    for (const entry of entries) {
      if (!entry.id?.startsWith('anthropic@')) continue
      byLabel[entry.id.slice('anthropic@'.length)] = (entry.sections ?? [])
        .filter(s => s.type === 'metric' && typeof s.percent === 'number' && s.label)
        .map(s => ({ label: s.label as string, percent: s.percent as number, resets_at: s.resets_at ?? null }))
    }
    await update($, quota, () => byLabel)
  } catch {
    // Quota is a nicety beside the switch; the rows stay without it.
  }
}

/** "Session (5h)" → "5h", "Weekly (7d)" → "7d", "Fable (7d)" → "Fable". */
function short(label: string): string {
  const window = /^(Session|Weekly)\s*\(([^)]+)\)/.exec(label)?.[2]
  return window ?? label.replace(/\s*\(.*\)$/, '')
}

function until(iso: string | null, now: number): string {
  const at = iso ? Date.parse(iso) : NaN
  if (!Number.isFinite(at) || at <= now) return ''
  const mins = Math.round((at - now) / 60_000)
  if (mins < 60) return `${mins}m`
  const hours = Math.floor(mins / 60)
  return hours < 24 ? `${hours}h${mins % 60 ? `${mins % 60}m` : ''}` : `${Math.floor(hours / 24)}d${hours % 24 ? `${hours % 24}h` : ''}`
}

function bar(percent: number): string {
  const filled = Math.max(0, Math.min(8, Math.round(percent / 12.5)))
  return '▰'.repeat(filled) + '▱'.repeat(8 - filled)
}

async function ask($: Engine, label: string) {
  await update($, pending, () => ({ label, plan: null, ok: false }))
  try {
    const out = await cli($, ['account', 'switch', '--desktop', '--dry-run', '--', label])
    const plan = `${out.stdout}${out.stderr}`.trim()
    await update($, pending, p => (p?.label === label ? { label, plan, ok: out.exitCode === 0 } : p))
  } catch (err) {
    await update($, pending, p => (p?.label === label ? { label, plan: String(err), ok: false } : p))
  }
}

async function launch($: Engine, label: string) {
  const p = await read($, pending)
  const status = await read($, desktop)
  if (!p?.ok || p.label !== label || !status?.profiles.some(x => x.label === label && !x.active)) return
  const b = await resolveBin($)
  if (!b) return
  const log = LOG.replace(/^~/, await home($))
  const job = `${JOB_PREFIX}.${Date.now()}`
  const out = await $.process.run([
    '/bin/launchctl', 'submit', '-l', job, '-o', log, '-e', log, '--',
    '/bin/sh', '-c', JOB_SCRIPT, 'sh', job, b, 'account', 'switch', '--desktop', '--yes', '--', label,
  ])
  await update($, pending, () => null)
  if (out.exitCode !== 0) {
    await update($, problem, () => `launchctl submit failed: ${out.stderr.trim() || `exit ${out.exitCode}`}`)
    return
  }
  await update($, launched, () => label)
  $.ui.toast(`Switching Claude Desktop to ${label}; the app will quit and reopen. Log: ${LOG}`)
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    await $.command.register({
      name: COMMAND,
      description: 'Switch which account the Claude Desktop app is signed in as',
    })
    void refresh($)
    void loadQuota($)
    return next(e)
  })

  on('command.run', { command: COMMAND }, async $ => {
    await update($, pending, () => null)
    await refresh($)
    void loadQuota($)
    await $.ui.open({ id: PANE, title: 'Claude accounts' })
    return { text: 'Claude accounts pane opened.' }
  })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    const { Box, Text, Button } = $.ui.resolve(e)
    const status = await read($, desktop)
    const err = await read($, problem)
    const p = await read($, pending)
    const done = await read($, launched)
    const quotas = await read($, quota)
    const now = Date.now()

    if (done) {
      return (
        <Box flexDirection="column">
          <Text>Switching Claude Desktop to {done}. The app is quitting and will reopen.</Text>
          <Text dimColor>If it does not come back, read {LOG}.</Text>
        </Box>
      )
    }

    if (p) {
      return (
        <Box flexDirection="column" gap={1}>
          <Text bold>Switch Claude Desktop to {p.label}?</Text>
          <Text>Claude quits and reopens signed in as {p.label}. Every open session restarts, this one included.</Text>
          <Text dimColor>{p.plan ?? 'Checking the switch plan (dry run)…'}</Text>
          <Box flexDirection="row" gap={1}>
            {p.ok && (
              <Button key="go" variant="primary" onPress={() => launch($, p.label)}>
                Switch and reopen
              </Button>
            )}
            <Button key="cancel" onPress={() => update($, pending, () => null)}>
              Cancel
            </Button>
          </Box>
        </Box>
      )
    }

    return (
      <Box flexDirection="column" gap={1}>
        <Text bold>Claude Desktop</Text>
        {err && <Text color="red">{err}</Text>}
        {!err && !status && <Text dimColor>No Claude Desktop app on this machine.</Text>}
        {status && status.profiles.length === 0 && (
          <Text dimColor>No accounts saved yet. Add one with {"`ai-usagebar account add <label> --desktop`"}.</Text>
        )}
        {status?.profiles.map(prof => (
          <Box key={`row:${prof.label}`} flexDirection="column">
            <Box flexDirection="row" gap={1}>
              <Text bold={prof.active}>{prof.label}</Text>
              {prof.email && <Text dimColor>{prof.email}</Text>}
              {prof.active ? (
                <Text dimColor>active</Text>
              ) : (
                <Button key={`switch:${prof.label}`} onPress={() => ask($, prof.label)}>
                  Switch
                </Button>
              )}
            </Box>
            {quotas === null ? (
              <Text dimColor>  reading quota…</Text>
            ) : (quotas[prof.label] ?? []).length === 0 ? (
              <Text dimColor>  no quota reading</Text>
            ) : (
              <Box flexDirection="row" flexWrap="wrap" columnGap={3} paddingLeft={2}>
                {(quotas[prof.label] ?? []).map(m => (
                  <Box key={`q:${prof.label}:${m.label}`} flexDirection="row" gap={1}>
                    <Text dimColor>{short(m.label)}</Text>
                    <Text color={m.percent >= 90 ? 'red' : m.percent >= 70 ? 'yellow' : 'green'}>{bar(m.percent)}</Text>
                    <Text>{Math.round(m.percent)}%</Text>
                    {until(m.resets_at, now) && <Text dimColor>↻ {until(m.resets_at, now)}</Text>}
                  </Box>
                ))}
              </Box>
            )}
          </Box>
        ))}
        {status && <Text dimColor>Switching quits and reopens the app.</Text>}
        <Button key="refresh" plain dimColor label="Refresh" onPress={() => Promise.all([refresh($), loadQuota($)])} />
      </Box>
    )
  })
}
