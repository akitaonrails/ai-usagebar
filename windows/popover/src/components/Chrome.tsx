import type { ReactNode } from "react";
import MdiChevronDown from "~icons/mdi/chevron-down";
import MdiChevronLeft from "~icons/mdi/chevron-left";
import MdiChevronRight from "~icons/mdi/chevron-right";
import MdiCogOutline from "~icons/mdi/cog-outline";
import MdiConsole from "~icons/mdi/console";
import MdiMagnifyScan from "~icons/mdi/magnify-scan";
import MdiApple from "~icons/mdi/apple";
import MdiLoginVariant from "~icons/mdi/login-variant";
import MdiMicrosoftWindows from "~icons/mdi/microsoft-windows";
import MdiInformationOutline from "~icons/mdi/information-outline";
import MdiPower from "~icons/mdi/power";
import MdiRefresh from "~icons/mdi/refresh";
import MdiUpdate from "~icons/mdi/update";
import MdiRestore from "~icons/mdi/restore";
import MdiTune from "~icons/mdi/tune-variant";
import { Hint } from "@/components/Hint";
import { IconButton } from "@/components/IconButton";
import { TruncatedText } from "@/components/TruncatedText";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { Payload, PopoverStyle } from "@/lib/types";
import { m } from "@/paraglide/messages.js";
import { useI18n } from "@/lib/i18n";
import { nextUpdateLabel, sendCommand } from "../model.js";

interface TopBarProps {
  onBack: () => void;
  onDetect?: () => void;
  onReset?: () => void;
  resetArmed?: boolean;
  resetLabel?: string;
  title: string;
}

/**
 * PopoverTopBar: compact bar, centered headline, Back on the left, Reset — and Detect, on the
 * Customize screen — on the right. The outer columns reserve the wider side (two icon buttons
 * plus their gap), so the title stays centered on every screen.
 */
export function TopBar({ onBack, onDetect, onReset, resetArmed, resetLabel, title }: TopBarProps) {
  const resetHint = resetArmed ? m.click_again_to_confirm() : resetLabel;
  return (
    <div className="bar-glass grid shrink-0 grid-cols-[calc(2*var(--icon-btn)_+_var(--gap-controls))_1fr_calc(2*var(--icon-btn)_+_var(--gap-controls))] items-center p-[var(--panel-pad)]">
      <IconButton label={m.back()} onClick={onBack}>
        <MdiChevronLeft />
      </IconButton>
      <h1 className="m-0 min-w-0 text-center">
        <TruncatedText className="block w-full text-[length:var(--sz-header)] font-semibold">{title}</TruncatedText>
      </h1>
      <div className="flex items-center justify-end gap-[var(--gap-controls)]">
        {onReset && resetHint ? (
          <IconButton align="end" label={resetHint} data-armed={resetArmed ? "" : undefined} onClick={onReset}>
            <MdiRestore />
          </IconButton>
        ) : null}
        {onDetect ? (
          <IconButton align="end" label={m.detect_providers()} onClick={onDetect}>
            <MdiMagnifyScan />
          </IconButton>
        ) : null}
      </div>
    </div>
  );
}

interface FooterProps {
  notice?: { failed: boolean; text: string } | null;
  nowMs: number;
  optionsOpen: boolean;
  payload: Payload;
  popoverStyle: PopoverStyle;
  updatePending: boolean;
  onOpenAbout: () => void;
  onCheckUpdates: () => void;
  onOpenCustomize: () => void;
  onOpenSettings: () => void;
  onOptionsOpenChange: (open: boolean) => void;
}

/**
 * PopoverFooter: app identity + next-update countdown on the left, the Options ▾ capsule on the
 * right. A blue dot after the version says a newer build is waiting (the banner may be snoozed).
 * `notice` is the copy-image result: the footer's own small text, the meter's red when it
 * failed, on the same line as the Options capsule.
 */
export function Footer({
  notice,
  nowMs,
  optionsOpen,
  payload,
  popoverStyle,
  updatePending,
  onOpenAbout,
  onCheckUpdates,
  onOpenCustomize,
  onOpenSettings,
  onOptionsOpenChange,
}: FooterProps) {
  const { language } = useI18n();
  const nextLabel = nextUpdateLabel(payload, nowMs, language);
  const native = popoverStyle === "native";
  return (
    <footer className="bar-glass flex shrink-0 items-center gap-[var(--gap-controls)] p-[var(--panel-pad)]">
      <div className="flex min-w-0 flex-col text-[length:var(--sz-badge)] leading-[var(--leading-note)] text-label-2">
        <span className="flex items-center gap-[var(--gap-inline)]">
          {payload.version ? `AI Usage ${payload.version}` : "AI Usage"}
          {updatePending ? (
            <Hint align="start" content={m.update_available()}>
              <span
                aria-label={m.update_available()}
                className="inline-block size-[var(--dot-sm)] shrink-0 rounded-full bg-meter-blue"
                role="img"
              />
            </Hint>
          ) : null}
        </span>
        {native ? null : <span className="tabular-nums">{nextLabel}</span>}
      </div>
      <span className="min-w-[var(--gap-controls)] flex-1" />
      {notice ? (
        <span
          role="status"
          className={
            notice.failed
              ? "min-w-0 truncate text-[length:var(--sz-badge)] leading-[var(--leading-note)] text-meter-red"
              : "min-w-0 truncate text-[length:var(--sz-badge)] leading-[var(--leading-note)] text-label-2"
          }
        >
          {notice.text}
        </span>
      ) : null}
      <DropdownMenu modal={false} open={optionsOpen} onOpenChange={onOptionsOpenChange}>
        <DropdownMenuTrigger asChild>
          <button type="button" className="capsule-btn">
            {m.options()}
            <MdiChevronDown className="size-[var(--icon-inline)]" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" side="top" sideOffset={6}>
          {native ? null : <MenuItem icon={<MdiTune />} label={m.customize()} onSelect={onOpenCustomize} />}
          <MenuItem icon={<MdiCogOutline />} label={m.settings()} onSelect={onOpenSettings} />
          <DropdownMenuSeparator />
          <MenuItem icon={<MdiRefresh />} label={m.refresh()} onSelect={() => sendCommand("refresh")} />
          <MenuItem icon={<MdiMagnifyScan />} label={m.detect_providers()} onSelect={() => sendCommand("detect")} />
          <MenuItem icon={<MdiConsole />} label={m.open_tui()} onSelect={() => sendCommand("open-tui")} />
          <DropdownMenuSeparator />
          <MenuItem
            checked={payload.startupEnabled}
            icon={startupIcon(payload.os)}
            label={m.start_at_login()}
            onSelect={() => sendCommand("toggle-startup")}
          />
          <DropdownMenuSeparator />
          <MenuItem icon={<MdiUpdate />} label={m.check_for_updates()} onSelect={onCheckUpdates} />
          <MenuItem icon={<MdiInformationOutline />} label={m.about()} onSelect={onOpenAbout} />
          <MenuItem icon={<MdiPower />} label={m.quit()} onSelect={() => sendCommand("quit")} />
        </DropdownMenuContent>
      </DropdownMenu>
    </footer>
  );
}

interface MenuItemProps {
  checked?: boolean;
  icon: ReactNode;
  label: string;
  onSelect: () => void;
}

function startupIcon(os: string) {
  if (os === "macos") return <MdiApple />;
  if (os === "windows") return <MdiMicrosoftWindows />;
  return <MdiLoginVariant />;
}

function MenuItem({ checked, icon, label, onSelect }: MenuItemProps) {
  return (
    <DropdownMenuItem onSelect={onSelect}>
      {icon}
      <span className="flex-1">{label}</span>
      {checked ? <span aria-label={m.on()}>✓</span> : null}
    </DropdownMenuItem>
  );
}

interface ScreenCrossLinkRowProps {
  icon: ReactNode;
  subtitle: string;
  title: string;
  onClick: () => void;
}

/** ScreenCrossLinkRow: grouped card matching Settings/Customize rows (same pad + radius). */
export function ScreenCrossLinkRow({ icon, subtitle, title, onClick }: ScreenCrossLinkRowProps) {
  return (
    <button
      type="button"
      className="card-surface cross-link hover-card flex w-full items-center gap-[var(--row-gap)] px-[var(--card-pad)] py-[var(--pad-control)] text-left"
      onClick={onClick}
    >
      <span className="grid size-[var(--row-icon-box)] shrink-0 place-items-center text-label-2 [&_svg]:size-[var(--icon-menu)]">{icon}</span>
      <span className="flex min-w-0 flex-1 flex-col">
        <TruncatedText className="text-[length:var(--sz-header)] font-semibold">{title}</TruncatedText>
        <TruncatedText className="text-[length:var(--sz-badge)] text-label-2">{subtitle}</TruncatedText>
      </span>
      <MdiChevronRight className="size-[var(--icon-row)] shrink-0 text-label-3" />
    </button>
  );
}
