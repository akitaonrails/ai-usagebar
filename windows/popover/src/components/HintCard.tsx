import type { ReactNode } from "react";
import MdiClose from "~icons/mdi/close";

interface HintCardProps {
  buttonTitle: string;
  icon: ReactNode;
  message: string;
  title: string;
  onAction: () => void;
  onDismiss: () => void;
}

/**
 * DismissableHintCard: icon, title + message + small action button, ✕ in the corner. Type sizes
 * follow the metric rows (label / supporting) so the card reads like the rest of the dashboard.
 */
export function HintCard({ buttonTitle, icon, message, title, onAction, onDismiss }: HintCardProps) {
  return (
    <div className="card-surface flex items-start gap-[10px] px-[14px] py-3">
      <span className="grid size-5 shrink-0 place-items-center text-label-2 [&_svg]:size-4">{icon}</span>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="text-[length:var(--sz-label)] font-semibold">{title}</span>
        <span className="text-[length:var(--sz-support)] leading-[1.35] text-label-2">{message}</span>
        <button
          type="button"
          className="mt-1 h-6 w-fit rounded-[6px] bg-[var(--control-fill)] px-2.5 text-[length:var(--sz-support)] hover:bg-[var(--control-fill-hover)]"
          onClick={onAction}
        >
          {buttonTitle}
        </button>
      </div>
      <button
        type="button"
        aria-label="Dismiss"
        className="plain-btn grid size-4 shrink-0 place-items-center text-label-2"
        onClick={onDismiss}
      >
        <MdiClose className="size-3" />
      </button>
    </div>
  );
}
