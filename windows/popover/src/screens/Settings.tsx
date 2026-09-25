import { useEffect, useState, type ReactNode } from "react";
import MdiInformationOutline from "~icons/mdi/information-outline";
import MdiTune from "~icons/mdi/tune-variant";
import { ScreenCrossLinkRow } from "@/components/Chrome";
import { ShortcutRecorder } from "@/components/ShortcutRecorder";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import type { Card, Language, Layout, Payload } from "@/lib/types";
import { m } from "@/paraglide/messages.js";
import { useI18n } from "@/lib/i18n";
import { wheelScrollRef } from "@/lib/wheelScroll";
import { Hint } from "@/components/Hint";
import { TruncatedText } from "@/components/TruncatedText";
import { sendCommand, updateModeLabel, updateStatusLabel } from "../model.js";
import { Customize } from "./Customize";

export type SettingsTab = "general" | "providers" | "menu" | "preferences" | "alerts";

const SETTINGS_TABS: Array<[SettingsTab, () => string]> = [
  ["general", () => m.general()],
  ["providers", () => m.providers()],
  ["menu", () => m.menu()],
  ["preferences", () => m.preferences()],
  ["alerts", () => m.alerts()],
];

interface SettingsProps {
  layout: Layout;
  nowMs: number;
  payload: Payload;
  cards: Card[];
  onAlwaysShowPace: (on: boolean) => void;
  onUsageGoal: (on: boolean) => void;
  onLanguage: (language: Language) => void;
  onCheckUpdates: () => void;
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
  onPopoverStyle: (style: Layout["popoverStyle"]) => void;
}

/** Settings uses native tabs when selected, otherwise every available section stays in one page. */
export function Settings({
  layout,
  nowMs,
  payload,
  cards,
  onAlwaysShowPace,
  onUsageGoal,
  onLanguage,
  onCheckUpdates,
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
  onPopoverStyle,
}: SettingsProps) {
  const native = layout.popoverStyle === "native";
  const settingsTabs = native
    ? SETTINGS_TABS.filter(([name]) => payload.os === "macos" || (name !== "menu" && name !== "alerts"))
    : [];
  const { language } = useI18n();
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

  const updateStatus = updateStatusLabel(payload, nowMs, language);

  function onTabKeyDown(event: React.KeyboardEvent<HTMLButtonElement>, current: SettingsTab) {
    const index = settingsTabs.findIndex(([name]) => name === current);
    let next = index;
    if (event.key === "ArrowRight") next = (index + 1) % settingsTabs.length;
    else if (event.key === "ArrowLeft") next = (index + settingsTabs.length - 1) % settingsTabs.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = settingsTabs.length - 1;
    else return;
    event.preventDefault();
    const nextTab = settingsTabs[next][0];
    onTabChange(nextTab);
    document.getElementById(`settings-tab-${nextTab}`)?.focus();
  }

  return (
    <div className="flex flex-col gap-[var(--section-gap)] native-settings">
      {native ? (
        <div ref={wheelScrollRef} className="native-tabs settings-tabs" role="tablist" aria-label={m.settings()}>
          {settingsTabs.map(([name, label]) => (
            <button
              key={name}
              type="button"
              id={`settings-tab-${name}`}
              className="native-tab settings-tab"
              role="tab"
              aria-controls={`settings-panel-${name}`}
              aria-selected={tab === name}
              tabIndex={tab === name ? 0 : -1}
              onClick={() => onTabChange(name)}
              onKeyDown={(event) => onTabKeyDown(event, name)}
            >
              {label()}
            </button>
          ))}
        </div>
      ) : null}
      {native && tab === "providers" ? (
        <div id="settings-panel-providers" role="tabpanel" aria-labelledby="settings-tab-providers" className="flex flex-col gap-[var(--header-card-gap)]">
          <div className="flex items-center justify-between gap-2">
            <div className="section-title">{m.providers()}</div>
            {cards.length > 0 ? (
              <button type="button" className="text-[length:var(--sz-badge)] text-label-2 hover:text-foreground" onClick={onResetCustomization}>
                {resetArmed ? m.click_again_to_confirm() : m.reset_all_customization()}
              </button>
            ) : null}
          </div>
          {cards.length > 0 ? (
            <Customize embedded cards={cards} layout={layout} onOpen={onOpenProvider} onOpenSettings={onOpenCustomize} onReorder={onReorderProviders} onToggle={onToggleProvider} />
          ) : (
            <div className="card-surface flex items-center justify-between gap-2 p-3">
              <span className="text-label-2">{m.no_providers_detected()}</span>
              <button type="button" className="text-[var(--accent)]" onClick={() => sendCommand("detect")}>{m.detect_providers()}</button>
            </div>
          )}
        </div>
      ) : null}
      {!native || tab === "general" ? (
      <div id={native ? "settings-panel-general" : undefined} role={native ? "tabpanel" : undefined} aria-labelledby={native ? "settings-tab-general" : undefined}>
      <Section title={m.general()}>
        <SettingRow label={m.launch_at_login()}>
          <Switch
            checked={payload.startupEnabled}
            aria-label={m.launch_at_login()}
            onCheckedChange={() => sendCommand("toggle-startup")}
          />
        </SettingRow>
        <SettingRow hint={m.refresh_interval_hint()} label={m.refresh_every()}>
          <Picker
            options={[
              ["1", m["1_minute"]()],
              ["5", m["5_minutes"]()],
              ["10", m["10_minutes"]()],
            ]}
            value={String(payload.refreshMinutes)}
            onChange={(minutes) => sendCommand("set-refresh", { minutes: Number(minutes) })}
          />
        </SettingRow>
        <SettingRow hint={m.global_shortcut_hint()} label={m.global_shortcut()}>
          <ShortcutRecorder
            error={payload.shortcutError}
            value={payload.shortcut}
            onChange={(value) => sendCommand("set-shortcut", { value })}
          />
        </SettingRow>
        {payload.shortcutError ? (
          <div className="-mt-[var(--gap-stack)] px-[var(--card-pad)] pb-[var(--pad-control)] text-[length:var(--sz-badge)] text-meter-red">
            {payload.shortcutError}
          </div>
        ) : null}
      </Section>
      </div>
      ) : null}
      {payload.os === "macos" && (!native || tab === "alerts") ? (
        <div id={native ? "settings-panel-alerts" : undefined} role={native ? "tabpanel" : undefined} aria-labelledby={native ? "settings-tab-alerts" : undefined}>
        <Section title={m.notifications()}>
          <SettingRow hint={m.quota_alerts_hint()} label={m.quota_alerts()}>
            <Switch
              checked={payload.notificationsEnabled}
              aria-label={m.quota_alerts()}
              onCheckedChange={(on) => sendCommand("set-notifications-enabled", { value: on === true })}
            />
          </SettingRow>
          <SettingRow hint={m.alert_threshold_hint()} label={m.alert_threshold()}>
            <div className="flex items-center gap-1">
              <input
                type="number"
                min={1}
                max={100}
                step={1}
                inputMode="numeric"
                className="threshold-input"
                aria-label={m.alert_threshold()}
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
      {payload.os === "macos" && (!native || tab === "menu") ? (
        <div id={native ? "settings-panel-menu" : undefined} role={native ? "tabpanel" : undefined} aria-labelledby={native ? "settings-tab-menu" : undefined}>
        <Section title={m.menu_bar()}>
          <SettingRow hint={m.menu_bar_shows_hint()} label={m.menu_bar_shows()}>
            <Picker
              options={[["chart", m.chart()], ["logos", m.logos()]]}
              value={payload.menuBarChart ? "chart" : "logos"}
              onChange={(value) => sendCommand("set-menu-bar-chart", { value: value === "chart" })}
            />
          </SettingRow>
        </Section>
        </div>
      ) : null}
      {!native || tab === "preferences" ? (
      <div id={native ? "settings-panel-preferences" : undefined} role={native ? "tabpanel" : undefined} aria-labelledby={native ? "settings-tab-preferences" : undefined} className="flex flex-col gap-[var(--section-gap)]">
      <Section title={m.appearance()}>
        <SettingRow label={m.language()}>
          <Picker
            options={[["en", "English"], ["pt-BR", "Português"]]}
            value={language}
            onChange={onLanguage}
          />
        </SettingRow>
        <SettingRow label={m.theme()}>
          <Picker
            options={[
              ["system", m.system()],
              ["light", m.light()],
              ["dark", m.dark()],
            ]}
            value={layout.theme}
            onChange={onTheme}
          />
        </SettingRow>
        <SettingRow hint={m.popover_style_hint()} label={m.popover_style()}>
          <Picker
            options={[["classic", m.classic()], ["native", m.native()]]}
            value={layout.popoverStyle}
            onChange={onPopoverStyle}
          />
        </SettingRow>
        <SettingRow hint={m.time_format_hint()} label={m.time_format()}>
          <Picker
            options={[
              ["auto", m.auto()],
              ["12", m["12_hour"]()],
              ["24", m["24_hour"]()],
            ]}
            value={layout.timeFormat}
            onChange={onTimeFormat}
          />
        </SettingRow>
      </Section>
      <Section title={m.usage_display()}>
        <SettingRow hint={m.usage_goal_hint()} label={m.usage_goal()}>
          <Switch
            checked={layout.usageGoal}
            aria-label={m.usage_goal()}
            onCheckedChange={(on) => onUsageGoal(on === true)}
          />
        </SettingRow>
        <SettingRow hint={m.show_usage_as_hint()} label={m.show_usage_as()}>
          <Picker
            options={[
              ["used", m.used()],
              ["left", m.left()],
            ]}
            value={layout.showAs}
            onChange={onShowAs}
          />
        </SettingRow>
        <SettingRow hint={m.reset_times_hint()} label={m.reset_times()}>
          <Picker
            options={[
              ["countdown", m.countdown()],
              ["exact", m.exact_time()],
            ]}
            value={layout.resetTimes}
            onChange={onResetTimes}
          />
        </SettingRow>
        <SettingRow hint={m.always_show_pacing_hint()} label={m.always_show_pacing()}>
          <Switch
            checked={layout.alwaysShowPace}
            aria-label={m.always_show_pacing()}
            onCheckedChange={(on) => onAlwaysShowPace(on === true)}
          />
        </SettingRow>
      </Section>
      </div>
      ) : null}
      {/* Native shows Updates under General only; Classic keeps it at the end of the page. */}
      {!native || tab === "general" ? (
      <Section title={m.updates()}>
        <SettingRow hint={m.updates_hint()} label={m.updates()}>
          <Picker
            options={[
              ["auto", updateModeLabel("auto", language)],
              ["notify", updateModeLabel("notify", language)],
              ["off", updateModeLabel("off", language)],
            ]}
            value={payload.updates}
            onChange={(mode) => sendCommand("set-updates", { mode })}
          />
        </SettingRow>
        <div className="flex items-start gap-[var(--row-gap)] px-[var(--card-pad)] py-[var(--pad-control)]">
          <div className="flex min-w-0 flex-1 flex-col">
            <span>{m.check_for_updates_setting()}</span>
            <span className="text-[length:var(--sz-badge)] leading-[var(--leading-note)] break-words text-label-2 [overflow-wrap:anywhere]">
              {updateStatus}
            </span>
          </div>
          <button type="button" className="action-btn" onClick={onCheckUpdates}>
            {m.check_now()}
          </button>
        </div>
      </Section>
      ) : null}
      {!native ? <ScreenCrossLinkRow
        icon={<MdiTune />}
        subtitle={m.choose_what_s_visible_and_where()}
        title={m.customize()}
        onClick={onOpenCustomize}
      /> : null}
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
    <div className="flex items-center gap-[var(--row-gap)] px-[var(--card-pad)] py-[var(--pad-control)]">
      <span className="flex min-w-0 items-center gap-[var(--gap-inline)]">
        <TruncatedText className="min-w-0">{label}</TruncatedText>
        {hint ? <SettingHint label={label} text={hint} /> : null}
      </span>
      <span className="min-w-[var(--gap-controls)] flex-1" />
      {children}
    </div>
  );
}

function SettingHint({ label, text }: { label: string; text: string }) {
  return (
    <Hint align="start" content={text}>
      <button
        type="button"
        aria-label={`${m.about()} ${label}`}
        className="grid size-[var(--row-icon-box)] shrink-0 place-items-center text-label-3"
      >
        <MdiInformationOutline className="size-[var(--icon-row)]" />
      </button>
    </Hint>
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

  const selectedLabel = options.find(([optionValue]) => optionValue === value)?.[1] ?? value;

  return (
    <Select value={value} onValueChange={onValueChange}>
      <SelectTrigger className="max-w-[var(--picker-max)] min-w-0">
        <SelectValue className="min-w-0 flex-1">
          <TruncatedText className="block min-w-0 w-full">{selectedLabel}</TruncatedText>
        </SelectValue>
      </SelectTrigger>
      <SelectContent position="popper" align="end">
        {options.map(([optionValue, label]) => (
          <SelectItem key={optionValue} value={optionValue}>
            {label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
