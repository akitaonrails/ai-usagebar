import { useEffect, useState, type ReactNode } from "react";
import MdiInformationOutline from "~icons/mdi/information-outline";
import MdiTune from "~icons/mdi/tune-variant";
import { ScreenCrossLinkRow } from "@/components/Chrome";
import { ShortcutRecorder } from "@/components/ShortcutRecorder";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { Card, Language, Layout, Payload } from "@/lib/types";
import { useI18n } from "@/lib/i18n";
import { useBusyLabel } from "@/lib/useBusyLabel";
import { sendCommand, updateModeLabel, updateStatusLabel } from "../model.js";
import { Customize } from "./Customize";

export type SettingsTab = "general" | "providers" | "menu" | "preferences" | "alerts";

const SETTINGS_TABS: Array<[SettingsTab, string]> = [
  ["general", "General"],
  ["providers", "Providers"],
  ["menu", "Menu"],
  ["preferences", "Preferences"],
  ["alerts", "Alerts"],
];

interface SettingsProps {
  layout: Layout;
  nowMs: number;
  payload: Payload;
  cards: Card[];
  onAlwaysShowPace: (on: boolean) => void;
  onUsageGoal: (on: boolean) => void;
  onLanguage: (language: Language) => void;
  onOpenCustomize: () => void;
  onOpenProvider: (id: string) => void;
  onReorderProviders: (ids: string[]) => void;
  onToggleProvider: (id: string, on: boolean) => void;
  onResetCustomization: () => void;
  resetArmed: boolean;
  tab: SettingsTab;
  onTabChange: (tab: SettingsTab) => void;
  onResetTimes: (resetTimes: string) => void;
  onShowAs: (showAs: string) => void;
  onTheme: (theme: string) => void;
  onTimeFormat: (timeFormat: Layout["timeFormat"]) => void;
}

/** Settings screen with compact macOS tabs; Windows retains its section layout. */
export function Settings({
  layout,
  nowMs,
  payload,
  cards,
  onAlwaysShowPace,
  onUsageGoal,
  onLanguage,
  onOpenCustomize,
  onOpenProvider,
  onReorderProviders,
  onToggleProvider,
  onResetCustomization,
  resetArmed,
  tab,
  onTabChange,
  onResetTimes,
  onShowAs,
  onTheme,
  onTimeFormat,
}: SettingsProps) {
  const { language, t } = useI18n();
  const [busy, startBusy] = useBusyLabel();
  const [thresholdDraft, setThresholdDraft] = useState(String(payload.notificationsThreshold));
  useEffect(() => setThresholdDraft(String(payload.notificationsThreshold)), [payload.notificationsThreshold]);

  function saveThreshold() {
    const threshold = Number(thresholdDraft);
    if (Number.isInteger(threshold) && threshold >= 1 && threshold <= 100) {
      sendCommand("set-notifications-threshold", { value: threshold });
    } else {
      setThresholdDraft(String(payload.notificationsThreshold));
    }
  }

  const hostButton = updateButtonFor(payload.update);
  const updateButton = busy ? { ...hostButton, disabled: true, label: t(busy) } : { ...hostButton, label: t(hostButton.label) };
  // The button already says what is happening; the line keeps the last known state.
  const updateStatus = updateStatusLabel(payload, nowMs, language);
  const providerOptions: Array<[string, string]> = [
    ["highest", t("Highest consumption")],
    ...payload.entries
      .filter((entry) => !layout.hidden[entry.id])
      .map((entry): [string, string] => [entry.id, entry.displayName || entry.shortName || entry.id]),
  ];
  const focusedProvider = providerOptions.some(([id]) => id === payload.menuBarProvider)
    ? payload.menuBarProvider
    : "highest";

  function onUpdateClick() {
    startBusy(hostButton.cmd === "check-update" ? "Checking…" : "Updating…");
    sendCommand(hostButton.cmd);
  }

  function onTabKeyDown(event: React.KeyboardEvent<HTMLButtonElement>, current: SettingsTab) {
    const index = SETTINGS_TABS.findIndex(([name]) => name === current);
    let next = index;
    if (event.key === "ArrowRight") next = (index + 1) % SETTINGS_TABS.length;
    else if (event.key === "ArrowLeft") next = (index + SETTINGS_TABS.length - 1) % SETTINGS_TABS.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = SETTINGS_TABS.length - 1;
    else return;
    event.preventDefault();
    const nextTab = SETTINGS_TABS[next][0];
    onTabChange(nextTab);
    document.getElementById(`settings-tab-${nextTab}`)?.focus();
  }

  return (
    <div className="flex flex-col gap-[var(--section-gap)] mac-settings">
      {payload.os === "macos" ? (
        <div className="settings-tabs" role="tablist" aria-label={t("Settings")}>
          {SETTINGS_TABS.map(([name, label]) => (
            <button
              key={name}
              type="button"
              id={`settings-tab-${name}`}
              className="settings-tab"
              role="tab"
              aria-controls={`settings-panel-${name}`}
              aria-selected={tab === name}
              tabIndex={tab === name ? 0 : -1}
              onClick={() => onTabChange(name)}
              onKeyDown={(event) => onTabKeyDown(event, name)}
            >
              {t(label)}
            </button>
          ))}
        </div>
      ) : null}
      {payload.os === "macos" && tab === "providers" ? (
        <div id="settings-panel-providers" role="tabpanel" aria-labelledby="settings-tab-providers" className="flex flex-col gap-[var(--header-card-gap)]">
          <div className="flex items-center justify-between gap-2">
            <div className="section-title">{t("Providers")}</div>
            {cards.length > 0 ? (
              <button type="button" className="text-[length:var(--sz-badge)] text-label-2 hover:text-foreground" onClick={onResetCustomization}>
                {t(resetArmed ? "Click again to confirm" : "Reset All Customization")}
              </button>
            ) : null}
          </div>
          {cards.length > 0 ? (
            <Customize embedded cards={cards} layout={layout} onOpen={onOpenProvider} onOpenSettings={onOpenCustomize} onReorder={onReorderProviders} onToggle={onToggleProvider} />
          ) : (
            <div className="card-surface flex items-center justify-between gap-2 p-3">
              <span className="text-label-2">{t("No providers detected")}</span>
              <button type="button" className="text-[var(--accent)]" onClick={() => sendCommand("detect")}>{t("Detect Providers")}</button>
            </div>
          )}
        </div>
      ) : null}
      {payload.os !== "macos" || tab === "general" ? (
      <div id={payload.os === "macos" ? "settings-panel-general" : undefined} role={payload.os === "macos" ? "tabpanel" : undefined} aria-labelledby={payload.os === "macos" ? "settings-tab-general" : undefined}>
      <Section title={t("General")}>
        <SettingRow label={t("Launch at Login")}>
          <Switch
            checked={payload.startupEnabled}
            aria-label={t("Launch at Login")}
            onCheckedChange={() => sendCommand("toggle-startup")}
          />
        </SettingRow>
        <SettingRow hint={t("How often the tray fetches a fresh reading from each provider.")} label={t("Refresh Every")}>
          <Picker
            options={[
              ["1", t("1 minute")],
              ["5", t("5 minutes")],
              ["10", t("10 minutes")],
            ]}
            value={String(payload.refreshMinutes)}
            onChange={(minutes) => sendCommand("set-refresh", { minutes: Number(minutes) })}
          />
        </SettingRow>
        <SettingRow hint={t("Show or hide this popover from any app.")} label={t("Global Shortcut")}>
          <ShortcutRecorder
            error={payload.shortcutError}
            value={payload.shortcut}
            onChange={(value) => sendCommand("set-shortcut", { value })}
          />
        </SettingRow>
        {payload.shortcutError ? (
          <div className="-mt-1 px-3 pb-[var(--pad-control)] text-[length:var(--sz-badge)] text-meter-red">
            {payload.shortcutError}
          </div>
        ) : null}
      </Section>
      </div>
      ) : null}
      {payload.os === "macos" && tab === "alerts" ? (
        <div id="settings-panel-alerts" role="tabpanel" aria-labelledby="settings-tab-alerts">
        <Section title={t("Notifications")}>
          <SettingRow hint={t("System notifications for quota limits and expiring reset credits.")} label={t("Quota alerts")}>
            <Switch
              checked={payload.notificationsEnabled}
              aria-label={t("Quota alerts")}
              onCheckedChange={(on) => sendCommand("set-notifications-enabled", { value: on === true })}
            />
          </SettingRow>
          <SettingRow hint={t("Notify when a fresh usage reading reaches this percentage.")} label={t("Alert threshold")}>
            <div className="flex items-center gap-1">
              <input
                type="number"
                min={1}
                max={100}
                step={1}
                inputMode="numeric"
                className="h-7 w-14 rounded-[6px] border border-[var(--border)] bg-[var(--control-fill)] px-1.5 text-right tabular-nums"
                aria-label={t("Alert threshold")}
                value={thresholdDraft}
                onChange={(event) => setThresholdDraft(event.target.value)}
                onBlur={saveThreshold}
                onKeyDown={(event) => { if (event.key === "Enter") event.currentTarget.blur(); }}
              />
              <span className="text-label-2">%</span>
            </div>
          </SettingRow>
        </Section>
        </div>
      ) : null}
      {payload.os === "macos" && tab === "menu" ? (
        <div id="settings-panel-menu" role="tabpanel" aria-labelledby="settings-tab-menu">
        <Section title={t("Menu Bar")}>
          <SettingRow label={t("Show All Providers")}>
            <Switch
              checked={payload.menuBarShowAll}
              aria-label={t("Show All Providers")}
              onCheckedChange={(value) => sendCommand("set-menu-bar-show-all", { value: value === true })}
            />
          </SettingRow>
          <SettingRow label={t("Hide Usage Value")}>
            <Switch
              checked={payload.menuBarHideValue}
              aria-label={t("Hide Usage Value")}
              onCheckedChange={(value) => sendCommand("set-menu-bar-hide-value", { value: value === true })}
            />
          </SettingRow>
          <SettingRow label={t("Usage Window")}>
            <Picker
              options={[
                ["auto", t("Highest")],
                ["session", t("5-hour")],
                ["weekly", t("Weekly")],
                ["monthly", t("Monthly")],
              ]}
              value={payload.menuBarWindow}
              onChange={(value) => sendCommand("set-menu-bar-window", { value })}
            />
          </SettingRow>
          <SettingRow label={t("Chart Icon Only")}>
            <Switch
              checked={payload.menuBarChart}
              aria-label={t("Chart Icon Only")}
              onCheckedChange={(value) => sendCommand("set-menu-bar-chart", { value: value === true })}
            />
          </SettingRow>
          <SettingRow label={t("Focused Provider")}>
            <Picker
              options={providerOptions}
              value={focusedProvider}
              onChange={(value) => sendCommand("set-menu-bar-provider", { value })}
            />
          </SettingRow>
        </Section>
        </div>
      ) : null}
      {payload.os !== "macos" || tab === "preferences" ? (
      <div id={payload.os === "macos" ? "settings-panel-preferences" : undefined} role={payload.os === "macos" ? "tabpanel" : undefined} aria-labelledby={payload.os === "macos" ? "settings-tab-preferences" : undefined} className="flex flex-col gap-[var(--section-gap)]">
      <Section title={t("Appearance")}>
        <SettingRow label={t("Language")}>
          <Picker
            options={[["en", "English"], ["pt-BR", "Português (Brasil)"]]}
            value={language}
            onChange={onLanguage}
          />
        </SettingRow>
        <SettingRow label={t("Theme")}>
          <Picker
            options={[
              ["system", t("System")],
              ["light", t("Light")],
              ["dark", t("Dark")],
            ]}
            value={layout.theme}
            onChange={onTheme}
          />
        </SettingRow>
        <SettingRow hint={t("Auto follows the system clock. 12-hour and 24-hour pin exact reset times.")} label={t("Time Format")}>
          <Picker
            options={[
              ["auto", t("Auto")],
              ["12", t("12-hour")],
              ["24", t("24-hour")],
            ]}
            value={layout.timeFormat}
            onChange={onTimeFormat}
          />
        </SettingRow>
      </Section>
      <Section title={t("Usage Display")}>
        {payload.os === "macos" ? (
          <SettingRow hint={t("Show a goal based on time elapsed in each usage window. Monthly goals may be estimated.")} label={t("Usage goal")}>
            <Switch
              checked={layout.usageGoal}
              aria-label={t("Usage goal")}
              onCheckedChange={(on) => onUsageGoal(on === true)}
            />
          </SettingRow>
        ) : null}
        <SettingRow hint={t("Used fills the bar with what is spent. Left fills it with what remains.")} label={t("Show Usage As")}>
          <Picker
            options={[
              ["used", t("Used")],
              ["left", t("Left")],
            ]}
            value={layout.showAs}
            onChange={onShowAs}
          />
        </SettingRow>
        <SettingRow hint={t("Countdown reads “Resets in 6d”. Exact time reads the clock, like “today at 6:38 PM”.")} label={t("Reset Times")}>
          <Picker
            options={[
              ["countdown", t("Countdown")],
              ["exact", t("Exact time")],
            ]}
            value={layout.resetTimes}
            onChange={onResetTimes}
          />
        </SettingRow>
        <SettingRow hint={t("Show the pace note on every metric. Off, only rows near their limit show it.")} label={t("Always Show Pacing")}>
          <Switch
            checked={layout.alwaysShowPace}
            aria-label={t("Always Show Pacing")}
            onCheckedChange={(on) => onAlwaysShowPace(on === true)}
          />
        </SettingRow>
      </Section>
      </div>
      ) : null}
      {payload.os === "macos" ? null : (
      <Section title={t("Updates")}>
        <SettingRow hint={t("Automatic installs a release when it is found. Notify shows a banner. Off stops the hourly check.")} label={t("Updates")}>
          <Picker
            options={[
              ["auto", t(updateModeLabel("auto"))],
              ["notify", t(updateModeLabel("notify"))],
              ["off", t(updateModeLabel("off"))],
            ]}
            value={payload.updates}
            onChange={(mode) => sendCommand("set-updates", { mode })}
          />
        </SettingRow>
        <div className="flex items-start gap-[10px] px-3 py-[var(--pad-control)]">
          <div className="flex min-w-0 flex-1 flex-col">
            <span>{t("Check for Updates")}</span>
            <span className="text-[length:var(--sz-badge)] leading-[1.35] break-words text-label-2 [overflow-wrap:anywhere]">
              {updateStatus}
            </span>
          </div>
          <button
            type="button"
            className="h-6 shrink-0 rounded-[6px] bg-[var(--control-fill)] px-2.5 text-[length:var(--sz-support)] hover:bg-[var(--control-fill-hover)] disabled:opacity-60"
            disabled={updateButton.disabled}
            onClick={onUpdateClick}
          >
            {updateButton.label}
          </button>
        </div>
      </Section>
      )}
      {payload.os === "macos" ? null : <ScreenCrossLinkRow
        icon={<MdiTune />}
        subtitle={t("Choose what's visible and where")}
        title={t("Customize")}
        onClick={onOpenCustomize}
      />}
    </div>
  );
}

interface SectionProps {
  children: ReactNode;
  title: string;
}

function Section({ children, title }: SectionProps) {
  return (
    <div className="flex flex-col gap-[var(--header-card-gap)]">
      <div className="section-title">{title}</div>
      <div className="card-surface">{children}</div>
    </div>
  );
}

interface SettingRowProps {
  children: ReactNode;
  hint?: string;
  label: string;
}

function SettingRow({ children, hint, label }: SettingRowProps) {
  return (
    <div className="flex items-center gap-[10px] px-[var(--pad-control)] py-[var(--pad-control)]">
      <span className="flex min-w-0 items-center gap-1">
        <span className="truncate">{label}</span>
        {hint ? <SettingHint label={label} text={hint} /> : null}
      </span>
      <span className="min-w-2 flex-1" />
      {children}
    </div>
  );
}

function SettingHint({ label, text }: { label: string; text: string }) {
  const { t } = useI18n();
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label={`${t("About")} ${label}`}
          className="grid size-3.5 shrink-0 place-items-center border-0 bg-transparent p-0 text-label-3"
        >
          <MdiInformationOutline className="size-3.5" />
        </button>
      </TooltipTrigger>
      <TooltipContent
        align="start"
        arrowClassName="bg-[var(--surface)] fill-[var(--surface)]"
        className="setting-hint"
        collisionPadding={12}
        side="top"
        sideOffset={6}
      >
        {text}
      </TooltipContent>
    </Tooltip>
  );
}

interface PickerProps<T extends string> {
  options: Array<[value: T, label: string]>;
  value: T;
  onChange: (value: T) => void;
}

/** `.pickerStyle(.menu)`: a compact pull-down that reads like the macOS popup button. */
function Picker<T extends string>({ options, value, onChange }: PickerProps<T>) {
  // Radix reports a plain string; hand back the typed option it names so callers with a
  // union-typed setting need no cast.
  function onValueChange(next: string) {
    const match = options.find(([optionValue]) => optionValue === next);
    if (match) onChange(match[0]);
  }

  return (
    <Select value={value} onValueChange={onValueChange}>
      <SelectTrigger
        size="sm"
        className="h-[var(--control-h)] gap-1 rounded-[var(--radius-sm)] border-0 bg-[var(--control-fill)] px-2 py-0 text-[12px] shadow-none hover:bg-[var(--control-fill-hover)] focus-visible:ring-0 data-[size=sm]:h-[var(--control-h)] [&_svg]:size-3"
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent className="rounded-[var(--radius-sm)] border-0" position="popper" align="end">
        {options.map(([optionValue, label]) => (
          <SelectItem key={optionValue} className="py-1 text-[12px]" value={optionValue}>
            {label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

interface UpdateButton {
  cmd: string;
  disabled: boolean;
  label: string;
}

/**
 * "Check Now" only while nothing is known; once a release is found the same
 * button installs it, so the row never asks the user to check again for an
 * answer it already has.
 */
function updateButtonFor(update: Payload["update"]): UpdateButton {
  switch (update?.state) {
    case "checking":
      return { cmd: "check-update", disabled: true, label: "Checking…" };
    case "available":
      return { cmd: "install-update", disabled: false, label: "Update" };
    case "downloading":
    case "installing":
      return { cmd: "install-update", disabled: true, label: "Updating…" };
    case "failed":
      return { cmd: "install-update", disabled: false, label: "Try Again" };
    default:
      return { cmd: "check-update", disabled: false, label: "Check Now" };
  }
}
