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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { Payload } from "@/lib/types";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { nextUpdateLabel, sendCommand } from "../model.js";

interface TopBarProps {
  onBack: () => void;
  onReset?: () => void;
  resetArmed?: boolean;
  resetLabel?: string;
  title: string;
}

/** PopoverTopBar: compact bar, centered headline, Back on the left, Reset on the right. */
export function TopBar({ onBack, onReset, resetArmed, resetLabel, title }: TopBarProps) {
  const { t } = useI18n();
  return (
    <div className="bar-glass grid shrink-0 grid-cols-[28px_1fr_28px] items-center p-[var(--panel-pad)]">
      <button type="button" aria-label={t("Back")} className="circle-btn" title={t("Back")} onClick={onBack}>
        <MdiChevronLeft className="size-4" />
      </button>
      <h1 className="m-0 truncate text-center text-[13px] font-semibold">{title}</h1>
      {onReset ? (
        <button
          type="button"
          aria-label={resetArmed ? t("Click again to confirm") : resetLabel}
          className={cn("circle-btn", resetArmed && "bg-destructive text-white hover:bg-destructive")}
          title={resetArmed ? t("Click again to confirm") : resetLabel}
          onClick={onReset}
        >
          <MdiRestore className="size-[15px]" />
        </button>
      ) : (
        <span />
      )}
    </div>
  );
}

interface FooterProps {
  locked: boolean;
  nowMs: number;
  optionsOpen: boolean;
  payload: Payload;
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
 */
export function Footer({
  locked,
  nowMs,
  optionsOpen,
  payload,
  updatePending,
  onOpenAbout,
  onCheckUpdates,
  onOpenCustomize,
  onOpenSettings,
  onOptionsOpenChange,
}: FooterProps) {
  const { language, t } = useI18n();
  const nextLabel = nextUpdateLabel(payload, nowMs, language);
  return (
    <footer className="bar-glass flex shrink-0 items-center gap-2 p-[var(--panel-pad)]">
      <div className="flex min-w-0 flex-col text-[10px] leading-[14px] text-label-2">
        <span className="flex items-center gap-[5px]">
          {payload.version ? `AI Usage ${payload.version}` : "AI Usage"}
          {updatePending ? (
            <span
              aria-label={t("Update available")}
              className="inline-block size-[6px] shrink-0 rounded-full bg-meter-blue"
              role="img"
              title={t("Update available")}
            />
          ) : null}
        </span>
        {payload.os === "macos" ? null : (
          <button
            type="button"
            className="plain-btn tabular-nums"
            title={t("Refresh now")}
            onClick={() => !locked && sendCommand("refresh")}
          >
            {nextLabel}
          </button>
        )}
      </div>
      <span className="min-w-2 flex-1" />
      <DropdownMenu modal={false} open={optionsOpen} onOpenChange={onOptionsOpenChange}>
        <DropdownMenuTrigger asChild>
          <button type="button" className="capsule-btn">
            {t("Options")}
            <MdiChevronDown className="size-[13px]" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" side="top" sideOffset={6} className="min-w-[184px] rounded-[10px] border-0 p-[5px] shadow-lg">
          {payload.os === "macos" ? null : <MenuItem icon={<MdiTune />} label={t("Customize")} onSelect={onOpenCustomize} />}
          <MenuItem icon={<MdiCogOutline />} label={t("Settings")} onSelect={onOpenSettings} />
          <DropdownMenuSeparator />
          <MenuItem icon={<MdiRefresh />} label={t("Refresh")} onSelect={() => sendCommand("refresh")} />
          <MenuItem icon={<MdiMagnifyScan />} label={t("Detect Providers")} onSelect={() => sendCommand("detect")} />
          <MenuItem icon={<MdiConsole />} label={t("Open TUI")} onSelect={() => sendCommand("open-tui")} />
          <DropdownMenuSeparator />
          <MenuItem
            checked={payload.startupEnabled}
            icon={startupIcon(payload.os)}
            label={t("Start at Login")}
            onSelect={() => sendCommand("toggle-startup")}
          />
          <DropdownMenuSeparator />
          <MenuItem icon={<MdiUpdate />} label={t("Check for Updates…")} onSelect={onCheckUpdates} />
          <MenuItem icon={<MdiInformationOutline />} label={t("About")} onSelect={onOpenAbout} />
          <MenuItem destructive icon={<MdiPower />} label={t("Quit")} onSelect={() => sendCommand("quit")} />
        </DropdownMenuContent>
      </DropdownMenu>
    </footer>
  );
}

interface MenuItemProps {
  checked?: boolean;
  destructive?: boolean;
  icon: ReactNode;
  label: string;
  onSelect: () => void;
}

function startupIcon(os: string) {
  if (os === "macos") return <MdiApple />;
  if (os === "windows") return <MdiMicrosoftWindows />;
  return <MdiLoginVariant />;
}

function MenuItem({ checked, destructive, icon, label, onSelect }: MenuItemProps) {
  const { t } = useI18n();
  return (
    <DropdownMenuItem
      className={cn(
        "gap-2 rounded-[var(--radius-sm)] px-2 py-[5px] text-[13px] focus:bg-[var(--card)] focus:text-label-1 [&_svg]:size-[15px] [&_svg]:text-label-2 focus:[&_svg]:text-label-2",
        destructive && "text-destructive",
      )}
      variant={destructive ? "destructive" : "default"}
      onSelect={onSelect}
    >
      {icon}
      <span className="flex-1">{label}</span>
      {checked ? <span aria-label={t("On")}>✓</span> : null}
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
      className="card-surface cross-link flex w-full items-center gap-[10px] px-[var(--pad-control)] py-[var(--pad-control)] text-left"
      onClick={onClick}
    >
      <span className="grid size-[18px] shrink-0 place-items-center text-label-2 [&_svg]:size-[15px]">{icon}</span>
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="truncate text-[length:var(--sz-header)] font-semibold">{title}</span>
        <span className="truncate text-[length:var(--sz-badge)] text-label-2">{subtitle}</span>
      </span>
      <MdiChevronRight className="size-3.5 shrink-0 text-label-3" />
    </button>
  );
}
