/** Shapes produced by `src/model.js`; the JS side references these through JSDoc `@returns`. */

export interface RowPrefs {
  always: string[];
  demand: string[];
  off: Record<string, boolean>;
}

export type TimeFormat = "12" | "24" | "auto";

export interface Layout {
  alwaysShowPace: boolean;
  cardOrder: string[];
  collapsed: Record<string, boolean>;
  hidden: Record<string, boolean>;
  hideExtras: boolean;
  hintDismissed: boolean;
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
  key?: string;
  kind: "metric";
  label: string;
  leftPercent: number;
  reset: string;
  resetAt: string;
  severity: string;
  usedPercent: number;
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

export interface ResetsRow {
  available: number;
  key?: string;
  kind: "resets";
  label: string;
}

export type Row = BlockRow | MetricRow | ResetsRow | TextRow;

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
  resetCredits?: ResetCredits | null;
  id: string;
  plan: string;
  sections: Section[];
  shortName: string;
  stale: boolean;
  status: string;
}

export interface Payload {
  entries: Entry[];
  generatedAt: number;
  hostError: string;
  /** Host OS: macos, windows, or linux. */
  os: string;
  nextRefreshAt: number;
  primary: string;
  /** Host refresh interval; one of 1, 5 or 10. */
  refreshMinutes: number;
  shortcut: string;
  shortcutError: string;
  startupEnabled: boolean;
  version: string;
}

export type Screen = "customize" | "dashboard" | "provider" | "settings";
