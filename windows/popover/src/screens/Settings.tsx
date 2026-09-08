import type { ReactNode } from "react";
import MdiTune from "~icons/mdi/tune-variant";
import { ScreenCrossLinkRow } from "@/components/Chrome";
import { ShortcutRecorder } from "@/components/ShortcutRecorder";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import type { Layout, Payload } from "@/lib/types";
import { useBusyLabel } from "@/lib/useBusyLabel";
import { sendCommand, updateModeLabel, updateStatusLabel } from "../model.js";

interface SettingsProps {
  layout: Layout;
  nowMs: number;
  payload: Payload;
  onAlwaysShowPace: (on: boolean) => void;
  onDensity: (density: string) => void;
  onOpenCustomize: () => void;
  onResetTimes: (resetTimes: string) => void;
  onShowAs: (showAs: string) => void;
  onTheme: (theme: string) => void;
  onTimeFormat: (timeFormat: Layout["timeFormat"]) => void;
}

/** SettingsScreen: General / Appearance / Usage Display / Updates sections, then the Customize cross-link. */
export function Settings({
  layout,
  nowMs,
  payload,
  onAlwaysShowPace,
  onDensity,
  onOpenCustomize,
  onResetTimes,
  onShowAs,
  onTheme,
  onTimeFormat,
}: SettingsProps) {
  const [busy, startBusy] = useBusyLabel();

  const hostButton = updateButtonFor(payload.update);
  const updateButton = busy ? { ...hostButton, disabled: true, label: busy } : hostButton;
  // The button already says what is happening; the line keeps the last known state.
  const updateStatus = updateStatusLabel(payload, nowMs);

  function onUpdateClick() {
    startBusy(hostButton.cmd === "check-update" ? "Checking…" : "Updating…");
    sendCommand(hostButton.cmd);
  }

  return (
    <div className="flex flex-col gap-[var(--section-gap)]">
      <Section title="General">
        <SettingRow label="Launch at Login">
          <Switch
            checked={payload.startupEnabled}
            aria-label="Launch at Login"
            onCheckedChange={() => sendCommand("toggle-startup")}
          />
        </SettingRow>
        <SettingRow label="Refresh Every">
          <Picker
            options={[
              ["1", "1 minute"],
              ["5", "5 minutes"],
              ["10", "10 minutes"],
            ]}
            value={String(payload.refreshMinutes)}
            onChange={(minutes) => sendCommand("set-refresh", { minutes: Number(minutes) })}
          />
        </SettingRow>
        <SettingRow label="Global Shortcut">
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
      <Section title="Appearance">
        <SettingRow label="Theme">
          <Picker
            options={[
              ["system", "System"],
              ["light", "Light"],
              ["dark", "Dark"],
            ]}
            value={layout.theme}
            onChange={onTheme}
          />
        </SettingRow>
        <SettingRow label="Density">
          <Picker
            options={[
              ["regular", "Default"],
              ["compact", "Compact"],
            ]}
            value={layout.density}
            onChange={onDensity}
          />
        </SettingRow>
        <SettingRow label="Time Format">
          <Picker
            options={[
              ["auto", "Auto"],
              ["12", "12-hour"],
              ["24", "24-hour"],
            ]}
            value={layout.timeFormat}
            onChange={onTimeFormat}
          />
        </SettingRow>
      </Section>
      <Section title="Usage Display">
        <SettingRow label="Show Usage As">
          <Picker
            options={[
              ["used", "Used"],
              ["left", "Left"],
            ]}
            value={layout.showAs}
            onChange={onShowAs}
          />
        </SettingRow>
        <SettingRow label="Reset Times">
          <Picker
            options={[
              ["countdown", "Countdown"],
              ["exact", "Exact time"],
            ]}
            value={layout.resetTimes}
            onChange={onResetTimes}
          />
        </SettingRow>
        <SettingRow label="Always Show Pacing">
          <Switch
            checked={layout.alwaysShowPace}
            aria-label="Always Show Pacing"
            onCheckedChange={(on) => onAlwaysShowPace(on === true)}
          />
        </SettingRow>
      </Section>
      <Section title="Updates">
        <SettingRow label="Updates">
          <Picker
            options={[
              ["auto", updateModeLabel("auto")],
              ["notify", updateModeLabel("notify")],
              ["off", updateModeLabel("off")],
            ]}
            value={payload.updates}
            onChange={(mode) => sendCommand("set-updates", { mode })}
          />
        </SettingRow>
        <div className="flex items-start gap-[10px] px-3 py-[var(--pad-control)]">
          <div className="flex min-w-0 flex-1 flex-col">
            <span>Check for Updates</span>
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
      <ScreenCrossLinkRow
        icon={<MdiTune />}
        subtitle="Choose what's visible and where"
        title="Customize"
        onClick={onOpenCustomize}
      />
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
  label: string;
}

function SettingRow({ children, label }: SettingRowProps) {
  return (
    <div className="flex items-center gap-[10px] px-3 py-[var(--pad-control)]">
      <span>{label}</span>
      <span className="min-w-2 flex-1" />
      {children}
    </div>
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
        className="h-6 gap-1 rounded-[6px] border-0 bg-[var(--control-fill)] px-2 text-[12px] shadow-none hover:bg-[var(--control-fill-hover)] focus-visible:ring-0 [&_svg]:size-3"
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent className="rounded-[8px]" position="popper" align="end">
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
