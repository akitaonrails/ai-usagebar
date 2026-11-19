import { useRef, type KeyboardEvent } from "react";
import { m } from "@/paraglide/messages.js";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { accountLabel } from "../model.js";

interface AccountTabsProps {
  /** The provider's accounts, in the dashboard's order. */
  cards: Array<{ id: string; title: string }>;
  /** The provider's display name; an "… · label" title loses its prefix. */
  providerName: string;
  /** The tab currently shown; a value no account holds falls back to the first one. */
  selected: string;
  onSelect: (id: string) => void;
}

/**
 * The account selector for a provider with several accounts: a segmented
 * control under the card's header, one tab per account — the default account
 * reads the catalog's "Default", an "openai@work" account reads "work" (#454).
 * Arrow keys and Home/End move the selection; with one account the caller
 * doesn't draw it at all, so the single-account screen is the old one.
 */
export function AccountTabs({ cards, providerName, selected, onSelect }: AccountTabsProps) {
  const { language } = useI18n();
  const refs = useRef(new Map<string, HTMLButtonElement>());
  const ids = cards.map((card) => String(card.id)).filter(Boolean);
  const active = ids.indexOf(selected) >= 0 ? selected : ids[0];

  function select(id: string) {
    refs.current.get(id)?.focus();
    if (id !== active) onSelect(id);
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const index = ids.indexOf(active);
    if (index < 0) return;
    let next: number | null = null;
    if (event.key === "ArrowRight") next = (index + 1) % ids.length;
    else if (event.key === "ArrowLeft") next = (index - 1 + ids.length) % ids.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = ids.length - 1;
    if (next === null) return;
    event.preventDefault();
    select(ids[next]);
  }

  return (
    <div
      className="flex gap-[var(--space-2xs)] rounded-[var(--radius-full)] bg-[var(--control-fill)] p-[var(--card-gutter)]"
      onKeyDown={onKeyDown}
      role="tablist"
      aria-label={m.accounts()}
    >
      {ids.map((id) => {
        const card = cards.find((item) => String(item.id) === id);
        const isSelected = id === active;
        return (
          <button
            key={id}
            ref={(node) => {
              if (node) refs.current.set(id, node);
              else refs.current.delete(id);
            }}
            type="button"
            role="tab"
            aria-selected={isSelected}
            tabIndex={isSelected ? 0 : -1}
            className={cn(
              "account-tab min-w-0 flex-1 rounded-[var(--radius-full)] px-[var(--space-xl)] py-[var(--space-sm)] text-[length:var(--sz-body)] outline-none",
              isSelected
                ? "bg-[var(--menu)] font-semibold text-foreground shadow-[var(--knob-shadow)]"
                : "text-label-2 hover:text-foreground",
            )}
            onClick={() => select(id)}
          >
            <span className="truncate">{card ? accountLabel(card, providerName, language) : ""}</span>
          </button>
        );
      })}
    </div>
  );
}
