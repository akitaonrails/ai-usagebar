import type { ReactNode } from "react";
import MdiClose from "~icons/mdi/close";
import { Hint } from "@/components/Hint";
import { TruncatedText } from "@/components/TruncatedText";
import { m } from "@/paraglide/messages.js";

interface HintCardProps {
  actionDisabled?: boolean;
  buttonTitle: string;
  dismissTitle?: string;
  icon: ReactNode;
  message: string;
  title: string;
  onAction: () => void;
  /** Omit to hide the ✕ (a card that must stay while its action runs). */
  onDismiss?: () => void;
}

/**
 * DismissableHintCard: icon and title on one line with the ✕ at the end, then the message and a
 * small action button on the card's left edge. Type sizes follow the metric rows (label /
 * supporting) so the card reads like the rest of the dashboard.
 */
export function HintCard({
  actionDisabled,
  buttonTitle,
  dismissTitle,
  icon,
  message,
  title,
  onAction,
  onDismiss,
}: HintCardProps) {
  return (
    <div className="card-surface flex flex-col items-start gap-[var(--gap-stack)] p-[var(--card-pad)]">
      <div className="flex w-full items-center gap-[var(--gap-item)]">
        <span className="grid shrink-0 place-items-center text-label-2 [&_svg]:size-[var(--icon-row)]">{icon}</span>
        <TruncatedText className="min-w-0 flex-1 text-[length:var(--sz-label)] font-semibold">{title}</TruncatedText>
        {onDismiss ? (
          <Hint align="end" content={dismissTitle}>
            <button
              type="button"
              aria-label={m.dismiss()}
              className="plain-btn hover-icon grid size-[var(--dismiss-box)] shrink-0 place-items-center text-label-2"
              onClick={onDismiss}
            >
              <MdiClose className="size-[var(--icon-dismiss)]" />
            </button>
          </Hint>
        ) : null}
      </div>
      <span className="text-[length:var(--sz-support)] leading-[var(--leading-note)] text-label-2">{message}</span>
      <button type="button" className="action-btn mt-[var(--gap-stack)]" disabled={actionDisabled} onClick={onAction}>
        {buttonTitle}
      </button>
    </div>
  );
}
