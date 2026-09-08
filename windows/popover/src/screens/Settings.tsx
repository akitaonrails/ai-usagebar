import type { ReactNode } from "react";
import MdiTune from "~icons/mdi/tune-variant";
import { ScreenCrossLinkRow } from "@/components/Chrome";
import { ShortcutRecorder } from "@/components/ShortcutRecorder";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import type { Layout, Payload } from "@/lib/types";
import { sendCommand } from "../model.js";

interface SettingsProps {
  layout: Layout;
  payload: Payload;
  onAlwaysShowPace: (on: boolean) => void;
  onDensity: (density: string) => void;
  onOpenCustomize: () => void;
  onResetTimes: (resetTimes: string) => void;
  onShowAs: (showAs: string) => void;
  onTheme: (theme: string) => void;
  onTimeFormat: (timeFormat: Layout["timeFormat"]) => void;
}

/** SettingsScreen: General / Appearance / Usage Display sections, then the Customize cross-link. */
export function Settings({
  layout,
  payload,
  onAlwaysShowPace,
  onDensity,
  onOpenCustomize,
  onResetTimes,
  onShowAs,
  onTheme,
  onTimeFormat,
}: SettingsProps) {
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
