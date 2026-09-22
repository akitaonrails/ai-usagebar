import type { ReactNode } from "react";
import MdiInformationOutline from "~icons/mdi/information-outline";
import MdiTune from "~icons/mdi/tune-variant";
import { ScreenCrossLinkRow } from "@/components/Chrome";
import { ShortcutRecorder } from "@/components/ShortcutRecorder";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { Layout, Payload } from "@/lib/types";
import { useBusyLabel } from "@/lib/useBusyLabel";
import { sendCommand, updateModeLabel, updateStatusLabel } from "../model.js";

interface SettingsProps {
  layout: Layout;
  nowMs: number;
  payload: Payload;
  onAlwaysShowPace: (on: boolean) => void;
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
        <SettingRow hint="How often the tray fetches a fresh reading from each provider." label="Refresh Every">
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
        <SettingRow hint="Show or hide this popover from any app." label="Global Shortcut">
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
        <SettingRow hint="Auto follows the system clock. 12-hour and 24-hour pin exact reset times." label="Time Format">
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
        <SettingRow hint="Used fills the bar with what is spent. Left fills it with what remains." label="Show Usage As">
          <Picker
            options={[
              ["used", "Used"],
              ["left", "Left"],
            ]}
            value={layout.showAs}
            onChange={onShowAs}
          />
        </SettingRow>
        <SettingRow hint="Countdown reads “Resets in 6d”. Exact time reads the clock, like “today at 6:38 PM”." label="Reset Times">
          <Picker
            options={[
              ["countdown", "Countdown"],
              ["exact", "Exact time"],
            ]}
            value={layout.resetTimes}
            onChange={onResetTimes}
          />
        </SettingRow>
        <SettingRow hint="Show the pace note on every metric. Off, only rows near their limit show it." label="Always Show Pacing">
          <Switch
            checked={layout.alwaysShowPace}
            aria-label="Always Show Pacing"
            onCheckedChange={(on) => onAlwaysShowPace(on === true)}
          />
        </SettingRow>
      </Section>
      {payload.os === "macos" ? null : (
      <Section title="Updates">
        <SettingRow hint="Automatic installs a release when it is found. Notify shows a banner. Off stops the hourly check." label="Updates">
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
      )}
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
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label={`About ${label}`}
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
