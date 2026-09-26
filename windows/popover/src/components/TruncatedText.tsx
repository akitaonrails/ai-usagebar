import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/** Tracks whether a single-line element's text is clipped. `text` re-measures when it changes. */
export function useClipped<T extends HTMLElement>(text: string) {
  const ref = useRef<T>(null);
  const [clipped, setClipped] = useState(false);

  const measure = useCallback(() => {
    const element = ref.current;
    if (!element) return;
    const next = element.scrollWidth > element.clientWidth;
    setClipped((current) => current === next ? current : next);
  }, []);

  useEffect(() => {
    const element = ref.current;
    if (!element) return;

    measure();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(element);
    window.addEventListener("resize", measure);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [text, measure]);

  return { clipped, measure, ref };
}

/** Hint body for text that may be clipped: the full text first when it is, then `hint`. */
export function clippedHint(text: string, clipped: boolean, hint?: ReactNode): ReactNode {
  if (!clipped) return hint || "";
  if (!hint) return text;
  return (
    <>
      <span className="block">{text}</span>
      <span className="block">{hint}</span>
    </>
  );
}

interface TruncatedTextProps {
  align?: "center" | "end" | "start";
  children: string;
  className?: string;
  /** Shown whether or not the text is clipped (an exact reset time, say). */
  hint?: ReactNode;
}

/**
 * Single-line text that shows its full value in the app hint while clipped, plus `hint` if given.
 * The hint stays mounted whatever the measurement says: swapping the element in and out of a
 * trigger would remount it under the pointer, and an outer Hint cannot wrap this component
 * (it forwards no trigger props), so extra text goes through `hint` instead.
 */
export function TruncatedText({ align = "center", children, className, hint }: TruncatedTextProps) {
  const { clipped, measure, ref } = useClipped<HTMLSpanElement>(children);
  const [open, setOpen] = useState(false);
  const content = clippedHint(children, clipped, hint);

  return (
    <Tooltip open={open && content !== ""} onOpenChange={setOpen}>
      <TooltipTrigger asChild>
        <span
          ref={ref}
          className={cn("block truncate", className)}
          // Reachable by Tab only while clipped, so the keyboard can open the full text too.
          tabIndex={clipped ? 0 : undefined}
          onFocus={measure}
          onPointerEnter={measure}
        >
          {children}
        </span>
      </TooltipTrigger>
      <TooltipContent align={align}>{content}</TooltipContent>
    </Tooltip>
  );
}
