import type { ReactNode } from "react";
import { Popover, PopoverArrow, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import type { TimeFormat } from "@/lib/types";
import { expirySeverity, formatDuration, formatResetExact } from "../model.js";

export interface ResetEvent {
  atMs: number;
  title?: string;
}

interface ResetPopoverProps {
  children: ReactNode;
  events: ResetEvent[];
  nowMs: number;
  timeFormat?: TimeFormat;
}

/**
 * Click-opened timeline of upcoming resets, matching OpenUsage's
 * RateLimitResetsDetail: numbered dots on a rail, exact time, countdown.
 */
export function ResetPopover({ children, events, nowMs, timeFormat }: ResetPopoverProps) {
  const sorted = events
    .filter((event) => Number.isFinite(event.atMs))
    .slice()
    .sort((a, b) => a.atMs - b.atMs);
  return (
    <Popover>
      <PopoverTrigger asChild>{children}</PopoverTrigger>
      <PopoverContent onOpenAutoFocus={(event) => event.preventDefault()}>
        <ResetTimeline events={sorted} nowMs={nowMs} timeFormat={timeFormat} />
        <PopoverArrow />
      </PopoverContent>
    </Popover>
  );
}

interface ResetTimelineProps {
  events: ResetEvent[];
  nowMs: number;
  timeFormat?: TimeFormat;
}

export function ResetTimeline({ events, nowMs, timeFormat }: ResetTimelineProps) {
  if (events.length === 0) {
    return (
      <div className="py-2 text-center text-[length:var(--sz-support)] text-label-2">
        You have no rate limit resets
      </div>
    );
  }
  return (
    <ol className="m-0 flex list-none flex-col p-0">
      {events.map((event, index) => {
        const severity = expirySeverity(event.atMs, nowMs);
        const last = index === events.length - 1;
        return (
          <li key={`${event.atMs}-${index}`} className="flex items-stretch gap-2.5">
            <span className="flex w-[18px] shrink-0 flex-col items-center">
              <span
                className={cn(
                  "grid size-[18px] place-items-center rounded-full text-[11px] font-medium",
                  severity === "red" && "bg-[var(--red)] text-white",
                  severity === "yellow" && "bg-[var(--yellow)] text-black",
                  severity === "blue" && "bg-[var(--blue)] text-white",
                )}
              >
                {index + 1}
              </span>
              {last ? null : <span className="w-[1.5px] min-h-[10px] flex-1 bg-border" />}
            </span>
            <span className={cn("flex min-w-0 flex-1 items-baseline gap-2", last ? "pb-0" : "pb-2.5")}>
              <span className="min-w-0 truncate text-[length:var(--sz-support)]">
                {formatResetExact(event.atMs, nowMs, { timeFormat })}
              </span>
              <span className="min-w-2 flex-1" />
              <span className="shrink-0 text-[length:var(--sz-support)] tabular-nums text-label-2">
                {formatDuration(event.atMs - nowMs)}
              </span>
            </span>
          </li>
        );
      })}
    </ol>
  );
}
