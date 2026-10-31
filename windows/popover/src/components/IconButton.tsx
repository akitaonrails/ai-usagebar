import { forwardRef, type ButtonHTMLAttributes } from "react";
import { Hint } from "@/components/Hint";
import { cn } from "@/lib/utils";

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** The accessible name; it is also the tooltip content, so an icon button never ships silent. */
  label: string;
  /** Tooltip alignment, forwarded to the Hint. */
  align?: "center" | "end" | "start";
}

/**
 * The one icon button of the popover: the uniform `.icon-btn` box (24 px, 16 px glyph,
 * label-2 at rest, hover fill, pressed scale) with the tooltip and the aria-label taken from
 * `label`. The ref and the remaining `<button>` attributes pass through, which is what lets
 * it sit in a Radix trigger (`DropdownMenuTrigger asChild`) and take explicit handlers;
 * `aria-label` and `data-slot="tooltip-trigger"` are applied after the rest spread so an
 * aria-label or a trigger marker coming through it (e.g. from a `DropdownMenuTrigger asChild`
 * chain) can never replace the required `label` or the tooltip-trigger marker of the real
 * Hint below.
 */
export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { align, children, className, label, type = "button", ...rest },
  ref,
) {
  return (
    <Hint align={align} content={label}>
      <button
        ref={ref}
        type={type}
        className={cn("icon-btn", className)}
        {...rest}
        aria-label={label}
        data-slot="tooltip-trigger"
      >
        {children}
      </button>
    </Hint>
  );
});
