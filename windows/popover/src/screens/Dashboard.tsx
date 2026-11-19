import MdiTune from "~icons/mdi/tune-variant";
import { HintCard } from "@/components/HintCard";
import { ErrorRow, ProviderSection } from "@/components/ProviderSection";
import type { RowAction } from "@/components/RowMenu";
import { UpdateBanner } from "@/components/UpdateBanner";
import { SortableItem, VerticalDnd } from "@/components/dnd";
import type { Card, Layout, Payload } from "@/lib/types";
import { m } from "@/paraglide/messages.js";
import { useI18n } from "@/lib/i18n";
import { activeAccountTab, accountSwitchFor, expandGroupOrder, explainError, providerGroups, updateBannerPending } from "../model.js";
import { Hint } from "@/components/Hint";

interface DashboardProps {
  cards: Card[];
  hint: boolean;
  layout: Layout;
  nowMs: number;
  payload: Payload;
  visible: Card[];
  onCustomizeProvider: (id: string) => void;
  onCopyImage: (id: string) => void;
  onDismissHint: () => void;
  onOpenCustomize: () => void;
  onReorder: (ids: string[]) => void;
  onRowAction: (providerId: string, rowKey: string, action: RowAction) => void;
  onRowMenuOpenChange: (open: boolean) => void;
  /** The provider's tab changed accounts; remembers the choice per provider. */
  onSelectAccount: (providerId: string, cardId: string) => void;
  onSwitchAccount: (vendor: string, label: string) => void;
  onToggleCollapse: (id: string) => void;
  onToggleShowAs: () => void;
}

interface DashboardBannersProps {
  compact?: boolean;
  hint: boolean;
  payload: Payload;
  onDismissHint: () => void;
  onOpenCustomize: () => void;
}

/** The shared update and first-run banners shown above either dashboard layout. */
export function DashboardBanners({ compact, hint, payload, onDismissHint, onOpenCustomize }: DashboardBannersProps) {
  if (payload.hostError) return null;
  const spacing = compact ? undefined : "mb-[var(--section-gap)]";
  return (
    <>
      {hint ? (
        <div className={spacing}>
          <HintCard
            buttonTitle={m.open_customize()}
            icon={<MdiTune />}
            message={m.welcome_hint()}
            title={m.welcome_to_ai_usage()}
            onAction={onOpenCustomize}
            onDismiss={onDismissHint}
          />
        </div>
      ) : null}
      {payload.update && updateBannerPending(payload) ? (
        <div className={spacing}>
          <UpdateBanner repository={payload.repository} update={payload.update} />
        </div>
      ) : null}
    </>
  );
}

/** DashboardContentView: provider sections stacked with the section gap. A provider with several
 * accounts is one draggable group with a tab per account; the card shows the tab's account. */
export function Dashboard({
  cards,
  hint,
  layout,
  nowMs,
  payload,
  visible,
  onCustomizeProvider,
  onCopyImage,
  onDismissHint,
  onOpenCustomize,
  onReorder,
  onRowAction,
  onRowMenuOpenChange,
  onSelectAccount,
  onSwitchAccount,
  onToggleCollapse,
  onToggleShowAs,
}: DashboardProps) {
  const { language } = useI18n();
  if (payload.hostError) {
    return (
      <Hint align="start" content={payload.hostError}>
        <div className="card-surface py-[var(--card-gutter)]">
          <ErrorRow explained={explainError(payload.hostError, undefined, language)} />
        </div>
      </Hint>
    );
  }
  if (visible.length === 0) {
    return (
      <>
        <DashboardBanners hint={hint} payload={payload} onDismissHint={onDismissHint} onOpenCustomize={onOpenCustomize} />
        <p className="m-0 px-[var(--space-2xl)] py-[var(--space-3xl)] text-center text-[length:var(--sz-support)] text-label-2">
          {cards.length
            ? m.customize_prompt_hint()
            : m.no_providers_enabled_hint()}
        </p>
      </>
    );
  }
  const groups = providerGroups(visible);
  // Decision 2: the group names itself with the catalog's name; a card without
  // a catalog row (a custom provider) keeps the card's title.
  const nameOf = (group: { id: string; cards: Card[] }) =>
    payload.providers?.find((provider) => provider.id === group.id)?.name ?? group.cards[0].title;
  const activeCardOf = (group: { id: string; cards: Card[] }) => {
    const id = activeAccountTab(group.cards, layout, group.id, payload.primary);
    return group.cards.find((card) => card.id === id) ?? group.cards[0];
  };
  return (
    <>
    <DashboardBanners hint={hint} payload={payload} onDismissHint={onDismissHint} onOpenCustomize={onOpenCustomize} />
    <VerticalDnd
      items={groups.map((group) => group.id)}
      onReorder={(groupIds) => onReorder(expandGroupOrder(groups, groupIds))}
      overlay={(id) => {
        const group = groups.find((item) => item.id === id);
        if (!group) return null;
        const card = activeCardOf(group);
        return (
          <ProviderSection
            card={card}
            group={{ name: nameOf(group), cards: group.cards.map((item) => ({ id: item.id, title: item.title })), activeId: card.id, onSelect: () => {} }}
            layout={layout}
            lifted
            nowMs={nowMs}
          />
        );
      }}
    >
      <div className="flex flex-col gap-[var(--section-gap)]">
        {groups.map((group) => {
          const card = activeCardOf(group);
          const name = nameOf(group);
          return (
            <SortableItem key={group.id} id={group.id}>
              {({ attributes, listeners }) => (
                <ProviderSection
                  account={accountSwitchFor(card.id, payload.accounts)}
                  card={card}
                  group={{ name, cards: group.cards.map((item) => ({ id: item.id, title: item.title })), activeId: card.id, onSelect: (cardId) => onSelectAccount(group.id, cardId) }}
                  handle={{ attributes, listeners }}
                  layout={layout}
                  nowMs={nowMs}
                  onCustomize={() => onCustomizeProvider(card.id)}
                  onCopyImage={() => onCopyImage(card.id)}
                  onRowAction={(key, action) => onRowAction(card.id, key, action)}
                  onRowMenuOpenChange={onRowMenuOpenChange}
                  onSwitchAccount={() => {
                    const account = accountSwitchFor(card.id, payload.accounts);
                    if (account) onSwitchAccount(account.vendor, account.label);
                  }}
                  onToggleCollapse={() => onToggleCollapse(card.id)}
                  onToggleShowAs={onToggleShowAs}
                />
              )}
            </SortableItem>
          );
        })}
      </div>
    </VerticalDnd>
    </>
  );
}
