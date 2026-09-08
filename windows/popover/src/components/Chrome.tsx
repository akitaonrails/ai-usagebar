import type { ReactNode } from "react";
import MdiChevronDown from "~icons/mdi/chevron-down";
import MdiChevronLeft from "~icons/mdi/chevron-left";
import MdiChevronRight from "~icons/mdi/chevron-right";
import MdiCogOutline from "~icons/mdi/cog-outline";
import MdiConsole from "~icons/mdi/console";
import MdiMagnifyScan from "~icons/mdi/magnify-scan";
import MdiMicrosoftWindows from "~icons/mdi/microsoft-windows";
import MdiPower from "~icons/mdi/power";
import MdiRefresh from "~icons/mdi/refresh";
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
import { cn } from "@/lib/utils";
import { nextUpdateLabel, sendCommand } from "../model.js";

interface TopBarProps {
  onBack: () => void;
  onReset?: () => void;
  resetArmed?: boolean;
  resetLabel?: string;
  title: string;
}

/** PopoverTopBar: 44pt bar, centered headline, circular Back on the left, circular Reset on the right. */
export function TopBar({ onBack, onReset, resetArmed, resetLabel, title }: TopBarProps) {
  return (
    <div className="bar-glass grid h-[var(--topbar-h)] shrink-0 grid-cols-[28px_1fr_28px] items-center px-[var(--panel-pad)]">
      <button type="button" aria-label="Back" className="circle-btn" title="Back" onClick={onBack}>
        <MdiChevronLeft className="size-4" />
      </button>
      <h1 className="m-0 truncate text-center text-[13px] font-semibold">{title}</h1>
      {onReset ? (
        <button
          type="button"
          aria-label={resetArmed ? "Click again to confirm" : resetLabel}
          className={cn("circle-btn", resetArmed && "bg-destructive text-white hover:bg-destructive")}
          title={resetArmed ? "Click again to confirm" : resetLabel}
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
  onOpenCustomize,
  onOpenSettings,
  onOptionsOpenChange,
}: FooterProps) {
  const nextLabel = nextUpdateLabel(payload, nowMs);
  return (
    <footer className="bar-glass flex shrink-0 items-center gap-2 px-[var(--panel-pad)] py-3">
      <div className="flex min-w-0 flex-col text-[10px] leading-[14px] text-label-2">
        <span className="flex items-center gap-[5px]">
          {payload.version ? `AI Usage ${payload.version}` : "AI Usage"}
          {updatePending ? (
            <span
              aria-label="Update available"
              className="inline-block size-[6px] shrink-0 rounded-full bg-meter-blue"
              role="img"
              title="Update available"
            />
          ) : null}
        </span>
        <button
          type="button"
          className="plain-btn tabular-nums"
          title="Refresh now"
          onClick={() => !locked && sendCommand("refresh")}
        >
          {nextLabel}
        </button>
      </div>
      <span className="min-w-2 flex-1" />
      <DropdownMenu modal={false} open={optionsOpen} onOpenChange={onOptionsOpenChange}>
        <DropdownMenuTrigger asChild>
          <button type="button" className="capsule-btn">
            Options
            <MdiChevronDown className="size-[13px]" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" side="top" sideOffset={6} className="min-w-[184px] rounded-[10px] p-[5px] shadow-lg">
          <MenuItem icon={<MdiTune />} label="Customize" onSelect={onOpenCustomize} />
          <MenuItem icon={<MdiCogOutline />} label="Settings" onSelect={onOpenSettings} />
          <DropdownMenuSeparator />
          <MenuItem icon={<MdiRefresh />} label="Refresh" onSelect={() => sendCommand("refresh")} />
          <MenuItem icon={<MdiMagnifyScan />} label="Detect Providers" onSelect={() => sendCommand("detect")} />
          <MenuItem icon={<MdiConsole />} label="Open TUI" onSelect={() => sendCommand("open-tui")} />
          <DropdownMenuSeparator />
          <MenuItem
            checked={payload.startupEnabled}
            icon={<MdiMicrosoftWindows />}
            label="Start with Windows"
            onSelect={() => sendCommand("toggle-startup")}
          />
          <MenuItem destructive icon={<MdiPower />} label="Quit" onSelect={() => sendCommand("quit")} />
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

function MenuItem({ checked, destructive, icon, label, onSelect }: MenuItemProps) {
  return (
    <DropdownMenuItem
      className={cn(
        "gap-2 rounded-[6px] px-2 py-[5px] text-[13px] focus:bg-primary focus:text-white [&_svg]:size-[15px] [&_svg]:text-label-2 focus:[&_svg]:text-white",
        destructive && "text-destructive",
      )}
      variant={destructive ? "destructive" : "default"}
      onSelect={onSelect}
    >
      {icon}
      <span className="flex-1">{label}</span>
      {checked ? <span aria-label="On">✓</span> : null}
    </DropdownMenuItem>
  );
}

interface ScreenCrossLinkRowProps {
  icon: ReactNode;
  subtitle: string;
  title: string;
  onClick: () => void;
}

/** ScreenCrossLinkRow: one card-row button that hops between Customize and Settings. */
export function ScreenCrossLinkRow({ icon, subtitle, title, onClick }: ScreenCrossLinkRowProps) {
  return (
    <button
      type="button"
      className="plain-btn card-surface flex w-full items-center gap-[10px] px-3 py-[var(--pad-control)] text-left"
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
