import * as React from "react";
import { Dialog as DialogPrimitive } from "radix-ui";

import { cn } from "@/lib/utils";

function Dialog({ ...props }: React.ComponentProps<typeof DialogPrimitive.Root>) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />;
}

/** A centered card over a scrim: the popover's inset and padding, the menus' radius and shadow. */
function DialogContent({ className, children, ...props }: React.ComponentProps<typeof DialogPrimitive.Content>) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay data-slot="dialog-overlay" className="fixed inset-0 z-50 bg-[var(--overlay)]" />
      <DialogPrimitive.Content
        data-slot="dialog-content"
        className={cn(
          "fixed top-1/2 left-1/2 z-50 flex w-[calc(100%-2*var(--panel-pad))] -translate-x-1/2 -translate-y-1/2 flex-col gap-[var(--section-gap)] rounded-[var(--card-radius)] bg-popover p-[var(--panel-pad)] text-popover-foreground shadow-[var(--menu-shadow)] outline-hidden",
          className,
        )}
        {...props}
      >
        {children}
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}

function DialogTitle({ className, ...props }: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn("m-0 text-[length:var(--sz-label)] font-semibold", className)}
      {...props}
    />
  );
}

function DialogDescription({ className, ...props }: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn(
        "m-0 text-[length:var(--sz-support)] leading-[var(--leading-note)] break-words text-label-2 [overflow-wrap:anywhere]",
        className,
      )}
      {...props}
    />
  );
}

/** Buttons, trailing edge, primary last (macOS alert order). */
function DialogFooter({ className, ...props }: React.ComponentProps<"div">) {
  return <div data-slot="dialog-footer" className={cn("flex justify-end gap-[var(--gap-controls)]", className)} {...props} />;
}

export { Dialog, DialogContent, DialogDescription, DialogFooter, DialogTitle };
