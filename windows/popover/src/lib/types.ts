/** Shapes produced by `src/model.js`; the JS side references these through JSDoc `@returns`. */

export interface RowPrefs {
  always: string[];
  demand: string[];
  off: Record<string, boolean>;
}

export type TimeFormat = "12" | "24" | "auto";
export type Language = "en" | "pt-BR" | "ko" | "es";
export type PopoverStyle = "classic" | "native";
export type MenuBarLook = "chart" | "logos" | "quattro";

export interface Layout {
  /** The account tab each provider's card shows (provider id → card id); the first card when absent. */
  accountTab: Record<string, string>;
  alwaysShowPace: boolean;
  usageGoal: boolean;
  cardOrder: string[];
  collapsed: Record<string, boolean>;
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
  /** The row sits under a group heading (SuperGrok's product slices, Claude's
   * CLI sessions) rather than being one of the provider's quota windows. */
  grouped?: boolean;
  key?: string;
  kind: "metric";
  /** One model's own limit (Antigravity's "Gemini 4 Argon") rather than a
   * window the whole provider shares; the report's `model_pool`. */
  modelPool?: boolean;
  label: string;
  leftPercent: number;
  /** What a refilling window (Synthetic's weekly credits) gets back at
   * `resetAt`, such as "2%"; the report's `regen`. Its countdown reads
   * "Regenerates 2% in …" instead of "Resets in …". */
  regen?: string;
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
  /** A host command, or one the popover runs itself: "add-key" (with `id`) or "customize". */
  cmd: string;
  /** The provider the key dialog opens for ("add-key" only). */
  id?: string;
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
  /** When the payload on screen was fetched (ms), or 0. */
  fetchedAt: number;
  /** Mark slug: the entry's `brand` when it declares one, else its vendor. */
  icon: string;
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
  brand: string;
  displayName: string;
  fetchedAt: number;
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
  prepareEnabled?: boolean;
  prepareTarget?: string;
  preparing?: boolean;
  prepareError?: string;
  preparedReset?: string;
}

/** The switch control on one account's card. */
export interface CardAccount {
  vendor: string;
  label: string;
  /** A Claude Desktop login: switching quits and reopens the app, so it asks twice. */
  desktop: boolean;
  active: boolean;
  /** A switch to this account is running. */
  switching: boolean;
  /** Another switch for this vendor is running, so this one must wait. */
  busy: boolean;
  /** Why the last switch to this account failed, or "". */
  error: string;
  prepareEnabled?: boolean;
  preparing?: boolean;
  prepareError?: string;
  preparedReset?: string;
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

/** One built-in provider from the host catalog, enabled or not. */
export interface ProviderInfo {
  /** "device" | "terminal" | "app" | "settings" | "": what Sign in does on this host. */
  action: "" | "app" | "device" | "settings" | "terminal";
  /** Account kinds this row can add (macOS): Claude Code, Claude Desktop, Codex. */
  addAccounts: Array<"code" | "codex" | "desktop">;
  configured: boolean;
  enabled: boolean;
  env: string;
  id: string;
  /** The credential's label for an API-key provider ("" for the others). */
  keyLabel?: string;
  /** The credential's note; "" when the host has none. */
  keyNote?: string;
  name: string;
  signIn: string;
  /** A device-code provider's account, for the sign-in sheet ("ChatGPT"); "" otherwise. */
  signInAccount?: string;
  /** Who hands out a device-code provider's code ("OpenAI"); "" otherwise. */
  signInIssuer?: string;
}

/** One row of the single provider list, as built by `providerListRows`. */
export interface ProviderListRow {
  /** The provider id for a catalog row, or the card id for a custom row. */
  id: string;
  /** The catalog's name (owned by the host), or the card title for a custom row. */
  name: string;
  /** The mark slug: the provider's icon, or the card's for a custom row. */
  icon: string;
  /** On in config.toml; a custom row is on by definition. */
  enabled: boolean;
  /** The switch draws (a catalog row); a custom row toggles in config.toml. */
  switchable: boolean;
  /** `providerState` of the catalog row; "" for a custom row. */
  state: string;
  /** The catalog's sign-in hint; the alert icon's tooltip while `state` is "needs". */
  signIn: string;
  action: "" | "app" | "device" | "settings" | "terminal";
  /** A device-code provider's account and code issuer, for the sign-in sheet; "" otherwise. */
  signInAccount: string;
  signInIssuer: string;
  /** The credential's label and note for a key-only provider ("API key"); "" otherwise. */
  keyLabel: string;
  keyNote: string;
  addAccounts: Array<"code" | "codex" | "desktop">;
  /** The provider's cards in the dashboard's order; a custom row carries its one. */
  cards: Array<{ id: string; metrics: number; title: string }>;
  /** The provider's total metric count across its accounts. */
  metrics: number;
}

/** The host's verdict for one key send, matched to it by `seq`. */
/** The host's device-code sign-in (`sign_in` in the payload); see `device_login.rs`. */
export interface SignInFact {
  /** The provider slug the flow runs for. */
  id: string;
  /** The one-time code to type on the provider's page; "" until there is one. */
  code: string;
  error: string;
  /** Wall-clock ms when the code stops working; 0 while unknown. */
  expiresAt: number;
  /** "device-off" | "cli-missing" | "custom-path" | "denied" | "". */
  reason: string;
  seq: number;
  state: "expired" | "failed" | "ok" | "starting" | "unavailable" | "waiting";
}

export interface KeyCheck {
  /** The provider slug the check ran for. */
  id: string;
  error: string;
  seq: number;
  state: "checking" | "failed" | "ok";
}

export interface Payload {
  /** Switchable logins keyed by vendor slug ("anthropic", "openai", "claude-desktop"); empty off macOS. */
  accounts: Record<string, AccountSwitchInfo>;
  accent: Accent | null;
  entries: Entry[];
  generatedAt: number;
  hostError: string;
  /** The host's last key-check verdict; null before the first send. */
  keyCheck: KeyCheck | null;
  signIn: SignInFact | null;
  /** macOS menu-bar look the host draws. */
  menuBarLook: MenuBarLook;
  /** Whether the Quattro look draws the short name beside the logo. */
  menuBarShortName: boolean;
  notificationsEnabled: boolean;
  notificationsThreshold: number;
  /** Host OS: macos, windows, or linux. */
  os: string;
  nextRefreshAt: number;
  primary: string;
  /** Every built-in provider, for Settings → Providers. */
  providers: ProviderInfo[];
  /** Why the last enable/disable failed, or "". */
  providersError: string;
  /** Host refresh interval; one of 1, 5, 10, 15, 30 or 60. */
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
