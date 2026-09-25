import * as React from "react"
import { Switch as SwitchPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

/** macOS `.switch` in `.controlSize(.small)`: `--switch-w` × `--switch-h` track, a `--switch-knob-w` ×
 * `--switch-knob` knob (round in Classic, a capsule in Native on macOS, WinUI's small dot on
 * Windows), one hairline border and a `--switch-knob-inset` on both ends — so the checked knob
 * travels the track width minus the knob, both borders and that inset (28 - 12 - 2 - 1 = 13px in
 * Classic), mirroring the unchecked inset. */
function Switch({ className, ...props }: React.ComponentProps<typeof SwitchPrimitive.Root>) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn(
        "peer group/switch inline-flex h-[var(--switch-h)] w-[var(--switch-w)] shrink-0 items-center rounded-full border-[length:var(--hairline)] border-transparent transition-colors outline-none focus-visible:ring-[length:var(--focus-ring)] focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:bg-primary data-[state=unchecked]:bg-input",
        className
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className="pointer-events-none block h-[var(--switch-knob)] w-[var(--switch-knob-w)] rounded-full bg-white shadow-[var(--knob-shadow)] ring-0 transition-transform data-[state=checked]:translate-x-[calc(var(--switch-w)-var(--switch-knob-w)-2*var(--hairline)-var(--switch-knob-inset))] data-[state=unchecked]:translate-x-[var(--switch-knob-inset)]"
      />
    </SwitchPrimitive.Root>
  )
}

export { Switch }
