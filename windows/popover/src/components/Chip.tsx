import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

/**
 * In-card controls. No borders.
 * - `link`: Status / Dashboard / Usage, flex-1, the height of every other control.
 * - `compact`: sized to its label and set in the row's own value size, for a control that
 *   sits inside a text row (the banked-resets count).
 */
const chipVariants = cva(
  "inline-flex items-center justify-center gap-[var(--gap-inline)] rounded-[var(--radius-sm)] bg-[var(--control-fill)] text-label-1 shadow-none outline-none ring-0 transition-colors hover:bg-[var(--control-fill-hover)] focus-visible:ring-0",
  {
    variants: {
      variant: {
        compact:
          "h-[var(--control-h-sm)] shrink-0 px-[var(--control-px-sm)] text-[length:var(--sz-support)]",
        link: "h-[var(--control-h)] min-w-0 flex-1 px-[var(--control-px)] text-[length:var(--sz-control)]",
      },
    },
    defaultVariants: { variant: "link" },
  },
);

const Chip = React.forwardRef<
  HTMLButtonElement,
  React.ComponentProps<"button"> & VariantProps<typeof chipVariants>
>(function Chip({ className, variant, type = "button", ...props }, ref) {
  return (
    <button ref={ref} type={type} data-slot="chip" className={cn(chipVariants({ variant }), className)} {...props} />
  );
});

export { Chip, chipVariants };
