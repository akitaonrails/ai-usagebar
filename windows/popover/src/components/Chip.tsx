import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

/**
 * Compact in-card controls. No borders.
 * - `link`: Status / Dashboard / Usage, flex-1.
 * - `count`: "2 available".
 */
const chipVariants = cva(
  "inline-flex items-center justify-center gap-1 rounded-[var(--radius-sm)] bg-[var(--control-fill)] text-[length:var(--sz-badge)] font-semibold text-label-1 shadow-none outline-none ring-0 transition-colors hover:bg-[var(--control-fill-hover)] focus-visible:ring-0",
  {
    variants: {
      variant: {
        link: "min-w-0 flex-1 px-2 py-[5px]",
        count: "px-2 py-[3px]",
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
    <button ref={ref} type={type} className={cn(chipVariants({ variant }), className)} {...props} />
  );
});

export { Chip, chipVariants };
