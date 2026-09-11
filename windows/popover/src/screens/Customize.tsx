import MdiChevronRight from "~icons/mdi/chevron-right";
import MdiCogOutline from "~icons/mdi/cog-outline";
import { ScreenCrossLinkRow } from "@/components/Chrome";
import { DragHandle } from "@/components/DragHandle";
import { ProviderIcon } from "@/components/ProviderIcon";
import { SortableItem, VerticalDnd } from "@/components/dnd";
import { Switch } from "@/components/ui/switch";
import type { Card, Layout } from "@/lib/types";
import { cn } from "@/lib/utils";
import { metricCount, orderedCards, providerIconId } from "../model.js";

interface CustomizeProps {
  cards: Card[];
  layout: Layout;
  onOpen: (id: string) => void;
  onOpenSettings: () => void;
  onReorder: (ids: string[]) => void;
  onToggle: (id: string, on: boolean) => void;
}

/** CustomizeProviderListView (L1): one grouped card of provider rows, then the Settings cross-link. */
export function Customize({ cards, layout, onOpen, onOpenSettings, onReorder, onToggle }: CustomizeProps) {
  const ordered = orderedCards(cards, layout);
  const ids = ordered.map((card) => card.id);
  return (
    <div className="flex flex-col gap-[var(--section-gap)]">
      <VerticalDnd
        items={ids}
        onReorder={onReorder}
        overlay={(id) => {
          const card = ordered.find((item) => item.id === id);
          if (!card) return null;
          return (
            <div className="lifted-surface">
              <ProviderListRow card={card} enabled />
            </div>
          );
        }}
      >
        <div className="card-surface">
          {ordered.map((card) => {
            const enabled = !(layout.hidden && layout.hidden[card.id]);
            return (
              <SortableItem key={card.id} id={card.id}>
                {({ attributes, listeners }) => (
                  <ProviderListRow
                    card={card}
                    enabled={enabled}
                    handle={{ attributes, listeners }}
                    onOpen={() => onOpen(card.id)}
                    onToggle={(on) => onToggle(card.id, on)}
                  />
                )}
              </SortableItem>
            );
          })}
        </div>
      </VerticalDnd>
      <ScreenCrossLinkRow
        icon={<MdiCogOutline />}
        subtitle="Windows startup, appearance and more"
        title="Settings"
        onClick={onOpenSettings}
      />
    </div>
  );
}

interface ProviderListRowProps {
  card: Card;
  enabled: boolean;
  handle?: Parameters<typeof DragHandle>[0];
  onOpen?: () => void;
  onToggle?: (on: boolean) => void;
}

/** ProviderListRow: grip, mark, name + "N metrics", switch, chevron. Disabled rows fade to 55%. */
function ProviderListRow({ card, enabled, handle, onOpen, onToggle }: ProviderListRowProps) {
  const count = metricCount(card);
  return (
    <div
      data-card-id={card.id}
      className={cn("flex items-center gap-[10px] px-3 py-[var(--pad-control)]", !enabled && "opacity-55")}
    >
      <DragHandle attributes={handle?.attributes} listeners={handle?.listeners} />
      <button type="button" className="plain-btn flex min-w-0 flex-1 items-center gap-[10px]" onClick={onOpen}>
        <ProviderIcon className="text-label-2" size={18} slug={providerIconId(card.id)} title={card.title} />
        <span className="flex min-w-0 flex-col">
          <span className="truncate text-[length:var(--sz-header)] font-semibold">{card.title}</span>
          <span className="text-[length:var(--sz-badge)] text-label-2">
            {count} {count === 1 ? "metric" : "metrics"}
          </span>
        </span>
      </button>
      <Switch checked={enabled} aria-label={`Show ${card.title}`} onCheckedChange={(on) => onToggle?.(on === true)} />
      <button type="button" aria-label={`Open ${card.title}`} className="plain-btn grid size-4 place-items-center" onClick={onOpen}>
        <MdiChevronRight className="size-3.5 text-label-3" />
      </button>
    </div>
  );
}
