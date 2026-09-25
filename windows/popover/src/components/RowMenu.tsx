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
import { m } from "@/paraglide/messages.js";

export type RowAction = "always" | "customize" | "demand" | "hide" | "refresh" | "star";

interface RowMenuProps {
  children: ReactNode;
  inAlways: boolean;
  providerTitle: string;
  starred: boolean;
  onAction: (action: RowAction) => void;
  onOpenChange: (open: boolean) => void;
}

/**
 * Right-click menu for one dashboard row: hide it, move it between Always Visible and On Demand,
 * or jump to the provider-level actions. The row itself is never the Radix trigger — a left click
 * on it must keep toggling the quota / reset readings — so the trigger is an invisible anchor
 * laid over the row, and `contextmenu` opens the menu programmatically.
 */
export function RowMenu({ children, inAlways, providerTitle, starred, onAction, onOpenChange }: RowMenuProps) {
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
      <DropdownMenuContent align="end" sideOffset={2}>
        <DropdownMenuItem onSelect={() => onAction("hide")}>
          <MdiEyeOff />
          <span className="flex-1">{m.hide_row()}</span>
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => onAction("star")}>
          {starred ? <MdiStar /> : <MdiStarOutline />}
          <span className="flex-1">{starred ? m.unstar_from_menu_bar() : m.star_for_menu_bar()}</span>
        </DropdownMenuItem>
        {inAlways ? (
          <DropdownMenuItem onSelect={() => onAction("demand")}>
            <MdiPinOff />
            <span className="flex-1">{m.show_on_demand()}</span>
          </DropdownMenuItem>
        ) : (
          <DropdownMenuItem onSelect={() => onAction("always")}>
            <MdiPin />
            <span className="flex-1">{m.always_show()}</span>
          </DropdownMenuItem>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => onAction("refresh")}>
          <MdiRefresh />
          <span className="flex-1">{m.refresh()} {providerTitle}</span>
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => onAction("customize")}>
          <MdiTune />
          <span className="flex-1">{m.customize()} {providerTitle}</span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
