import { useEffect, useState, type ReactNode } from "react";
import MdiEyeOff from "~icons/mdi/eye-off-outline";
import MdiPin from "~icons/mdi/pin-outline";
import MdiPinOff from "~icons/mdi/pin-off-outline";
import MdiRefresh from "~icons/mdi/refresh";
import MdiStar from "~icons/mdi/star";
import MdiStarOutline from "~icons/mdi/star-outline";
import MdiTune from "~icons/mdi/tune-variant";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useI18n } from "@/lib/i18n";

export type RowAction = "always" | "customize" | "demand" | "hide" | "refresh" | "star";

interface RowMenuProps {
  children: ReactNode;
  inAlways: boolean;
  providerTitle: string;
  starred: boolean;
  onAction: (action: RowAction) => void;
  onOpenChange: (open: boolean) => void;
}

/** Same item chrome as the footer Options menu (Chrome.MenuItem). */
const ITEM_CLASS =
  "gap-2 rounded-[var(--radius-sm)] px-2 py-[5px] text-[13px] focus:bg-[var(--card)] focus:text-label-1 [&_svg]:size-[15px] [&_svg]:text-label-2 focus:[&_svg]:text-label-2";

/**
 * Right-click menu for one dashboard row: hide it, move it between Always Visible and On Demand,
 * or jump to the provider-level actions. The row itself is never the Radix trigger — a left click
 * on it must keep toggling the quota / reset readings — so the trigger is an invisible anchor
 * laid over the row, and `contextmenu` opens the menu programmatically.
 */
export function RowMenu({ children, inAlways, providerTitle, starred, onAction, onOpenChange }: RowMenuProps) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    onOpenChange(true);
    // The popover hides itself on focus loss; a menu left open would still be there next time.
    const close = () => setOpen(false);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("blur", close);
      onOpenChange(false);
    };
  }, [open, onOpenChange]);

  return (
    <DropdownMenu modal={false} open={open} onOpenChange={setOpen}>
      <div
        className="relative"
        onContextMenu={(event) => {
          event.preventDefault();
          setOpen(true);
        }}
      >
        {children}
        <DropdownMenuTrigger asChild>
          <span aria-hidden="true" className="pointer-events-none absolute inset-0" tabIndex={-1} />
        </DropdownMenuTrigger>
      </div>
      <DropdownMenuContent align="end" sideOffset={2} className="min-w-[184px] rounded-[10px] p-[5px] shadow-lg">
        <DropdownMenuItem className={ITEM_CLASS} onSelect={() => onAction("hide")}>
          <MdiEyeOff />
          <span className="flex-1">{t("Hide row")}</span>
        </DropdownMenuItem>
        <DropdownMenuItem className={ITEM_CLASS} onSelect={() => onAction("star")}>
          {starred ? <MdiStar /> : <MdiStarOutline />}
          <span className="flex-1">{t(starred ? "Unstar from menu bar" : "Star for menu bar")}</span>
        </DropdownMenuItem>
        {inAlways ? (
          <DropdownMenuItem className={ITEM_CLASS} onSelect={() => onAction("demand")}>
            <MdiPinOff />
            <span className="flex-1">{t("Show on demand")}</span>
          </DropdownMenuItem>
        ) : (
          <DropdownMenuItem className={ITEM_CLASS} onSelect={() => onAction("always")}>
            <MdiPin />
            <span className="flex-1">{t("Always show")}</span>
          </DropdownMenuItem>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem className={ITEM_CLASS} onSelect={() => onAction("refresh")}>
          <MdiRefresh />
          <span className="flex-1">{t("Refresh")} {providerTitle}</span>
        </DropdownMenuItem>
        <DropdownMenuItem className={ITEM_CLASS} onSelect={() => onAction("customize")}>
          <MdiTune />
          <span className="flex-1">{t("Customize")} {providerTitle}</span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
