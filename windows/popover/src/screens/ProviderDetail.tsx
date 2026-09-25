import {
  closestCorners,
  DndContext,
  DragOverlay,
  type DragEndEvent,
  type DragOverEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import { restrictToVerticalAxis } from "@dnd-kit/modifiers";
import { useRef, useState } from "react";
import { DragHandle } from "@/components/DragHandle";
import {
  handleRowDragEnd,
  handleRowDragOver,
  LIST_ALWAYS,
  LIST_DEMAND,
  SortableColumn,
  SortableItem,
  useTraySensors,
  type RowLists,
} from "@/components/dnd";
import MdiStar from "~icons/mdi/star";
import MdiStarOutline from "~icons/mdi/star-outline";
import { Switch } from "@/components/ui/switch";
import type { Card, Layout, Row } from "@/lib/types";
import { useI18n } from "@/lib/i18n";
import { isStarred, prefsForCard, rowKey } from "../model.js";

interface ProviderDetailProps {
  card?: Card;
  layout: Layout;
  starError?: string;
  onReorderRows: (lists: RowLists) => void;
  onToggleRow: (key: string, on: boolean) => void;
  onToggleStar: (key: string) => void;
}

/**
 * CustomizeProviderDetailView (L2): Always Visible and On Demand grouped cards. Rows drag within
 * a card or across the divider; an empty card shows the dashed "Drag metrics here" target.
 */
export function ProviderDetail({ card, layout, starError, onReorderRows, onToggleRow, onToggleStar }: ProviderDetailProps) {
  const { t } = useI18n();
  const sensors = useTraySensors();
  const [activeId, setActiveId] = useState<string | null>(null);
  const [draft, setDraft] = useState<RowLists | null>(null);
  const listsRef = useRef<RowLists>({ always: [], demand: [] });
  if (!card) return null;
  const prefs = prefsForCard(card, layout);
  const byKey = new Map<string, Row>((card.rows || []).map((row: Row) => [rowKey(row), row]));
  const lists: RowLists = draft || { always: prefs.always, demand: prefs.demand };
  listsRef.current = lists;

  function onDragStart(event: DragStartEvent) {
    const initial = { always: prefs.always.slice(), demand: prefs.demand.slice() };
    listsRef.current = initial;
    setActiveId(String(event.active.id));
    setDraft(initial);
  }

  function onDragOver(event: DragOverEvent) {
    const next = handleRowDragOver(event, listsRef.current);
    if (!next) return;
    listsRef.current = next;
    setDraft(next);
  }

  function onDragEnd(event: DragEndEvent) {
    const next = handleRowDragEnd(event, listsRef.current);
    setActiveId(null);
    setDraft(null);
    onReorderRows(next);
  }

  const overlayRow = activeId ? byKey.get(activeId) : undefined;

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCorners}
      modifiers={[restrictToVerticalAxis]}
      onDragStart={onDragStart}
      onDragOver={onDragOver}
      onDragEnd={onDragEnd}
      onDragCancel={() => {
        setActiveId(null);
        setDraft(null);
      }}
    >
      <div className="flex flex-col gap-[var(--section-gap)]">
        <MetricSection
          byKey={byKey}
          cardId={card.id}
          id={LIST_ALWAYS}
          keys={lists.always}
          layout={layout}
          prefs={prefs}
          title={t("Always Visible")}
          onToggleRow={onToggleRow}
          onToggleStar={onToggleStar}
        />
        <MetricSection
          byKey={byKey}
          cardId={card.id}
          id={LIST_DEMAND}
          keys={lists.demand}
          layout={layout}
          prefs={prefs}
          title={t("On Demand")}
          onToggleRow={onToggleRow}
          onToggleStar={onToggleStar}
        />
        {starError ? (
          <div className="px-[var(--space-xs)] text-[length:var(--sz-badge)] text-meter-red">{t(starError)}</div>
        ) : null}
      </div>
      <DragOverlay dropAnimation={null}>
        {overlayRow ? (
          <div className="lifted-surface">
            <MetricTuneRow enabled row={overlayRow} />
          </div>
        ) : null}
      </DragOverlay>
    </DndContext>
  );
}

interface MetricSectionProps {
  byKey: Map<string, Row>;
  cardId: string;
  id: string;
  keys: string[];
  layout: Layout;
  prefs: { off: Record<string, boolean> };
  title: string;
  onToggleRow: (key: string, on: boolean) => void;
  onToggleStar: (key: string) => void;
}

function MetricSection({
  byKey,
  cardId,
  id,
  keys,
  layout,
  prefs,
  title,
  onToggleRow,
  onToggleStar,
}: MetricSectionProps) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-[var(--header-card-gap)]">
      <div className="section-title">{title}</div>
      <SortableColumn id={id} items={keys} className="card-surface">
        {keys.length === 0 ? <div className="drop-zone">{t("Drag metrics here")}</div> : null}
        {keys.map((key) => {
          const row = byKey.get(key);
          if (!row) return null;
          return (
            <SortableItem key={key} id={key}>
              {({ attributes, listeners }) => (
                <MetricTuneRow
                  enabled={!prefs.off[key]}
                  handle={{ attributes, listeners }}
                  row={row}
                  starred={isStarred(layout.stars, cardId, key)}
                  onStar={() => onToggleStar(key)}
                  onToggle={(on) => onToggleRow(key, on)}
                />
              )}
            </SortableItem>
          );
        })}
      </SortableColumn>
    </div>
  );
}

interface MetricTuneRowProps {
  enabled: boolean;
  handle?: Parameters<typeof DragHandle>[0];
  row: Row;
  starred?: boolean;
  onStar?: () => void;
  onToggle?: (on: boolean) => void;
}

/** CustomizeMetricRow: grip, metric title, star, on/off switch. */
function MetricTuneRow({ enabled, handle, row, starred, onStar, onToggle }: MetricTuneRowProps) {
  const { metricLabel, t } = useI18n();
  const title = metricLabel(String(row.label || row.kind));
  return (
    <div data-row-key={rowKey(row)} className="flex items-center gap-[var(--row-gap)] px-[var(--card-pad)] py-[var(--pad-control)]">
      <DragHandle attributes={handle?.attributes} listeners={handle?.listeners} />
      <span className="min-w-0 flex-1 truncate">{title}</span>
      {onStar ? (
        <button
          type="button"
          aria-label={starred ? `${t("Unstar")} ${title}` : `${t("Star")} ${title} ${t("for menu bar")}`}
          aria-pressed={starred === true}
          className="hover-icon inline-flex size-[var(--row-icon-box)] items-center justify-center text-label-2 hover:text-foreground"
          onClick={onStar}
        >
          {starred ? <MdiStar className="size-[var(--icon-row)] text-primary" /> : <MdiStarOutline className="size-[var(--icon-row)]" />}
        </button>
      ) : null}
      <Switch checked={enabled} aria-label={`${t("Show")} ${title}`} onCheckedChange={(on) => onToggle?.(on === true)} />
    </div>
  );
}
