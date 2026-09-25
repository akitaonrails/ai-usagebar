import * as React from "react"
import { cn } from "@/lib/utils"
import { Tooltip as TooltipPrimitive } from "radix-ui"

function TooltipProvider({
  delayDuration = 0,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Provider>) {
  return (
    <TooltipPrimitive.Provider
      data-slot="tooltip-provider"
      delayDuration={delayDuration}
      {...props}
    />
  )
}

function Tooltip({
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Root>) {
  return <TooltipPrimitive.Root data-slot="tooltip" {...props} />
}

function TooltipTrigger({
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Trigger>) {
  return <TooltipPrimitive.Trigger data-slot="tooltip-trigger" {...props} />
}

/** Gap between a hint and its trigger, in px (Radix positions in JS, so not a CSS token). */
const HINT_OFFSET = 2

/**
 * The app's hover hint: --hint-bg / --hint-fg (near-black on the light theme, the window
 * surface on the dark one) with a soft shadow. No arrow; it sits just above its trigger.
 */
function TooltipContent({
  className,
  collisionPadding = 12,
  side = "top",
  sideOffset = HINT_OFFSET,
  children,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Content>) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        data-slot="tooltip-content"
        collisionPadding={collisionPadding}
        side={side}
        sideOffset={sideOffset}
        className={cn(
          "z-50 w-fit max-w-[var(--hint-max-w)] origin-(--radix-tooltip-content-transform-origin) rounded-[var(--radius-sm)] bg-[var(--hint-bg)] p-[var(--hint-pad)] text-left text-[length:var(--sz-support)] leading-[var(--leading-note)] font-normal text-[var(--hint-fg)] shadow-[var(--hint-shadow)]",
          className
        )}
        {...props}
      >
        {children}
      </TooltipPrimitive.Content>
    </TooltipPrimitive.Portal>
  )
}

export { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider }
