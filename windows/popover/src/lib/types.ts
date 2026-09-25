/** Shapes produced by `src/model.js`; the JS side references these through JSDoc `@returns`. */

export interface RowPrefs {
  always: string[];
  demand: string[];
  off: Record<string, boolean>;
}

export type TimeFormat = "12" | "24" | "auto";
export type Language = "en" | "pt-BR";
export type PopoverStyle = "classic" | "native";

export interface Layout {
  alwaysShowPace: boolean;
  usageGoal: boolean;
  cardOrder: string[];
  collapsed: Record<string, boolean>;
  hidden: Record<string, boolean>;
  hideExtras: boolean;
  hintDismissed: boolean;
  language: Language;
  popoverStyle: PopoverStyle;
  resetTimes: string;
  rows: Record<string, RowPrefs>;
  seeded: boolean;
  showAs: string;
  /** Provider id → starred metric keys (max 2). */
  stars: Record<string, string[]>;
  /** Menu-bar strip: compact bars glyph, or provider+values text. */
  stripStyle: "bars" | "text";
  theme: string;
  timeFormat: TimeFormat;
}

export interface MetricRow {
  /** The report's detail line; the hover text when `headline` is "value". */
  detail: string;
  /** Which number the headline shows: the percentage, or `value`. */
  headline: "percent" | "value";
  key?: string;
  kind: "metric";
  label: string;
  leftPercent: number;
  reset: string;
  resetAt: string;
  severity: string;
  usedPercent: number;
  /** The report's value text; the headline when `headline` is "value" (a money figure). */
  value: string;
  /** Reset window length in seconds; 0 when the host reports none. */
  window: number;
}

export type PaceState = "ahead" | "behind" | "onTrack";

export interface Pace {
  /** How much of the reset window has elapsed, 0..100. */
  elapsedPercent: number;
  projectedPercent: number;
  runsOutMs: number | null;
  sparePercent: number;
  state: PaceState;
}

export interface TextRow {
  key?: string;
  kind: "text";
  label: string;
  value: string;
}

export interface BlockRow {
  body: string[];
  key?: string;
  kind: "block";
  label: string;
}

export interface ResetCredit {
  expiresAt: string;
  title: string;
}

export interface ResetCredits {
  available: number;
  credits: ResetCredit[];
}

/** One line of a reset timeline: a banked credit's expiry, or a metric's next reset. */
export interface ResetItem {
  date: string;
  remaining: string;
  /** How soon a banked credit expires; "" for a metric reset or an unknown date. */
  severity: "" | "blue" | "red" | "yellow";
  title: string;
}

export interface ResetCreditsRow extends ResetCredits {
  key?: string;
  kind: "resetCredits";
  label: string;
}

export type Row = BlockRow | MetricRow | ResetCreditsRow | TextRow;

export interface ErrorAction {
  cmd: string;
  label: string;
}

export interface ExplainedError {
  action?: ErrorAction;
  hint: string;
  title: string;
}

export interface CardWarning {
  hint: string;
  raw: string;
  title: string;
}

export interface ResetCredit {
  expiresAt: string;
  title: string;
}

export interface ResetCredits {
  available: number;
  credits: ResetCredit[];
}

export interface Card {
  error: string;
  errorDetail: string;
  errorHint: string;
  errorTitle: string;
  id: string;
  plan: string;
  resetCredits: ResetCredits | null;
  rows: Row[];
  stale: boolean;
  title: string;
  warning: CardWarning | null;
}

export interface MetricSection {
  detail: string;
  /** Sub-group heading this metric renders under ("" when it stands alone). */
  group: string;
  headline: "percent" | "value";
  label: string;
  percent: number;
  resetAt: string;
  severity: string;
  type: "metric";
  value: string;
  /** Reset window length in seconds; 0 when the host reports none. */
  window: number;
}

export interface TextSection {
  label: string;
  type: "text";
  value: string;
}

export interface BlockSection {
  body: string[];
  label: string;
  type: "block";
}

export type Section = BlockSection | MetricSection | TextSection;

export interface Entry {
  displayName: string;
  error: string;
  id: string;
  plan: string;
  resetCredits: ResetCredits | null;
  sections: Section[];
  shortName: string;
  stale: boolean;
  status: string;
}

export type UpdateMode = "auto" | "notify" | "off";

export type UpdateState = "available" | "checking" | "downloading" | "failed" | "installing";

export interface UpdateInfo {
  error: string;
  /** The release ships this OS/arch and the install directory is writable. */
  installable: boolean;
  state: UpdateState;
  /** Release page; only a `https://github.com/` URL is kept, else "". */
  url: string;
  version: string;
}

/** One vendor's switchable logins, as the macOS host reports them. */
export interface AccountSwitchInfo {
  /** Label of the login in use, or "" when it is not a managed account. */
  active: string;
  labels: string[];
  /** Label of the last switch requested, running or finished. */
  target: string;
  switching: boolean;
  /** Why that switch failed, or "". */
  error: string;
}

/** The switch control on one account's card. */
export interface CardAccount {
  vendor: string;
  label: string;
  active: boolean;
  /** A switch to this account is running. */
  switching: boolean;
  /** Another switch for this vendor is running, so this one must wait. */
  busy: boolean;
  /** Why the last switch to this account failed, or "". */
  error: string;
}

/** What an update affordance does on click: a host command, or open `url`. */
export interface UpdateAction {
  busy: boolean;
  cmd: "" | "check-update" | "install-update" | "open-url";
  label: string;
  url: string;
}

export interface Accent {
  dark: string;
  light: string;
}

export interface Payload {
  /** Switchable logins keyed by vendor slug ("anthropic", "openai"); empty off macOS. */
  accounts: Record<string, AccountSwitchInfo>;
  accent: Accent | null;
  entries: Entry[];
  generatedAt: number;
  hostError: string;
  menuBarChart: boolean;
  notificationsEnabled: boolean;
  notificationsThreshold: number;
  /** Host OS: macos, windows, or linux. */
  os: string;
  nextRefreshAt: number;
  primary: string;
  /** Host refresh interval; one of 1, 5 or 10. */
  refreshMinutes: number;
  shortcut: string;
  shortcutError: string;
  startupEnabled: boolean;
  update: UpdateInfo | null;
  updateCheckedAt: number;
  updates: UpdateMode;
  /** GitHub repository this build was compiled from, or "". */
  repository: string;
  version: string;
}

export type Screen = "about" | "customize" | "dashboard" | "provider" | "settings";
