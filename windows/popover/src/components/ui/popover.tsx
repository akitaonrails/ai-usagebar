import * as React from "react";
import { Popover as PopoverPrimitive } from "radix-ui";

import { cn } from "@/lib/utils";

function Popover({ ...props }: React.ComponentProps<typeof PopoverPrimitive.Root>) {
  return <PopoverPrimitive.Root data-slot="popover" {...props} />;
}

function PopoverTrigger({ ...props }: React.ComponentProps<typeof PopoverPrimitive.Trigger>) {
  return <PopoverPrimitive.Trigger data-slot="popover-trigger" {...props} />;
}

function PopoverContent({
  className,
  align = "center",
  side = "top",
  sideOffset = 8,
  ...props
}: React.ComponentProps<typeof PopoverPrimitive.Content>) {
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Content
        data-slot="popover-content"
        align={align}
        side={side}
        sideOffset={sideOffset}
        className={cn(
          "z-50 w-[250px] origin-(--radix-popover-content-transform-origin) rounded-[18px] bg-popover p-2 text-popover-foreground shadow-[var(--lift-shadow)] outline-hidden",
          className,
        )}
        {...props}
      />
    </PopoverPrimitive.Portal>
  );
}

function PopoverArrow({ className, ...props }: React.ComponentProps<typeof PopoverPrimitive.Arrow>) {
  return (
    <PopoverPrimitive.Arrow
      data-slot="popover-arrow"
      width={16}
      height={8}
      className={cn("fill-popover", className)}
      {...props}
    />
  );
}

export { Popover, PopoverArrow, PopoverContent, PopoverTrigger };
