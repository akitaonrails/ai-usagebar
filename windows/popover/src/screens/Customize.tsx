import MdiCogOutline from "~icons/mdi/cog-outline";
import { ScreenCrossLinkRow } from "@/components/Chrome";
import { ProviderList } from "@/components/ProviderList";
import type { Card, Layout, Payload } from "@/lib/types";
import { m } from "@/paraglide/messages.js";

interface CustomizeProps {
  cards: Card[];
  layout: Layout;
  onOpen: (id: string) => void;
  onOpenSettings: () => void;
  onReorder: (ids: string[]) => void;
  onToggle: (id: string, on: boolean) => void;
  /** The host catalog: the one provider list the screen is built from. */
  payload: Payload;
}

/** Customize (L1): the single provider list, then the Settings cross-link. */
export function Customize({ cards, layout, onOpen, onOpenSettings, onReorder, onToggle, payload }: CustomizeProps) {
  return (
    <div className="flex flex-col gap-[var(--section-gap)]">
      <ProviderList
        heading
        cards={cards}
        error={payload.providersError}
        layout={layout}
        onOpen={onOpen}
        onReorder={onReorder}
        onToggle={onToggle}
        providers={payload.providers}
      />
      <ScreenCrossLinkRow
        icon={<MdiCogOutline />}
        subtitle={m.startup_appearance_and_more()}
        title={m.settings()}
        onClick={onOpenSettings}
      />
    </div>
  );
}
