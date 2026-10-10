export type DesktopProfile = {
  label: string
  email: string | null
  active: boolean
}

export type DesktopStatus = {
  profiles: DesktopProfile[]
  active_label: string | null
}

/** One quota window of an account, as `ai-usagebar usage --json` reports it. */
export type QuotaMetric = {
  label: string
  percent: number
  resets_at: string | null
}

/** The switch being confirmed: its label and the dry run's plan, once it has one. */
export type Pending = {
  label: string
  plan: string | null
  ok: boolean
}

declare module 'claude-code' {
  interface PluginState {
    'desktop-accounts': {
      /** null: no Claude Desktop app on this machine. */
      desktop: DesktopStatus | null
      problem: string
      pending: Pending | null
      launched: string
      /** Quota windows by account label; null until the first read lands. */
      quota: Record<string, QuotaMetric[]> | null
    }
  }
}
