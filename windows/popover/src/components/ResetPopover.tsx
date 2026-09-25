import type { ReactNode } from "react";
import { Hint } from "@/components/Hint";
import { TruncatedText } from "@/components/TruncatedText";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import type { ResetItem } from "@/lib/types";
import { cn } from "@/lib/utils";
import { m } from "@/paraglide/messages.js";
import { useI18n } from "@/lib/i18n";
import type { TimeFormat } from "@/lib/types";
import { expirySeverity, formatDuration, formatResetExact } from "../model.js";

export interface ResetEvent {
  atMs: number;
  title?: string;
}

interface ResetPopoverProps {
  children: ReactNode;
  hidden: number;
  items: ResetItem[];
}

/**
 * Click-opened list of banked rate-limit resets: a numbered dot colored by how soon each one
 * expires, the date, and the countdown. Same surface and row rhythm as the Options menu.
 */
export function ResetPopover({ children, hidden, items }: ResetPopoverProps) {
  return (
    <Popover>
      <PopoverTrigger asChild>{children}</PopoverTrigger>
      <PopoverContent
        align="end"
        className="text-[length:var(--sz-body)]"
        collisionPadding={12}
        onOpenAutoFocus={(event) => event.preventDefault()}
      >
        {items.length === 0 ? (
          <div className="px-[var(--menu-item-px)] py-[var(--menu-item-py)] text-label-2">{m.you_have_no_rate_limit_resets()}</div>
        ) : (
          <ol className="m-0 list-none p-0">
            {items.map((item, index) => (
              <Hint key={`${item.date}-${index}`} align="start" content={item.title}>
                <li className="flex items-center gap-[var(--gap-controls)] px-[var(--menu-item-px)] py-[var(--menu-item-py)]">
                  <span
                    className={cn(
                      "grid size-[var(--icon-card)] shrink-0 place-items-center rounded-full text-[length:var(--sz-badge)] font-medium tabular-nums",
                      item.severity === "red" && "bg-[var(--red)] text-white",
                      item.severity === "yellow" && "bg-[var(--yellow)] text-black",
                      item.severity === "blue" && "bg-[var(--blue)] text-white",
                      item.severity === "" && "bg-[var(--control-fill)] text-label-2",
                    )}
                  >
                    {index + 1}
                  </span>
                  <TruncatedText className="min-w-0 flex-1">{item.date}</TruncatedText>
                  <span className="shrink-0 tabular-nums text-label-2">{item.remaining}</span>
                </li>
              </Hint>
            ))}
          </ol>
        )}
        {hidden > 0 ? <div className="px-[var(--menu-item-px)] py-[var(--menu-item-py)] text-right text-label-2">{m.available_more({ count: hidden })}</div> : null}
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
  const { language } = useI18n();
  if (events.length === 0) {
    return (
      <div className="py-2 text-center text-[length:var(--sz-support)] text-label-2">
        {m.you_have_no_rate_limit_resets()}
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
              <TruncatedText className="min-w-0 text-[length:var(--sz-support)]">
                {formatResetExact(event.atMs, nowMs, { timeFormat, locale: language })}
              </TruncatedText>
              <span className="min-w-2 flex-1" />
              <span className="shrink-0 text-[length:var(--sz-support)] tabular-nums text-label-2">
                {formatDuration(event.atMs - nowMs, language)}
              </span>
            </span>
          </li>
        );
      })}
    </ol>
  );
}
