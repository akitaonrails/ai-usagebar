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
import { Switch } from "@/components/ui/switch";
import type { Card, Layout, Row } from "@/lib/types";
import { prefsForCard, rowKey } from "../model.js";

interface ProviderDetailProps {
  card?: Card;
  layout: Layout;
  onReorderRows: (lists: RowLists) => void;
  onToggleRow: (key: string, on: boolean) => void;
}

/**
 * CustomizeProviderDetailView (L2): Always Visible and On Demand grouped cards. Rows drag within
 * a card or across the divider; an empty card shows the dashed "Drag metrics here" target.
 */
export function ProviderDetail({ card, layout, onReorderRows, onToggleRow }: ProviderDetailProps) {
  const sensors = useTraySensors();
  const [activeId, setActiveId] = useState<string | null>(null);
  const [draft, setDraft] = useState<RowLists | null>(null);
  const listsRef = useRef<RowLists>({ always: [], demand: [] });
  if (!card) return null;
  const prefs = prefsForCard(card, layout);
  const byKey = new Map<string, Row>((card.rows || []).map((row: Row) => [rowKey(row), row]));
  const offKeys = Object.keys(prefs.off || {}).filter((key) => prefs.off[key] && byKey.has(key));
  const lists: RowLists = draft || { always: prefs.always, demand: prefs.demand.concat(offKeys) };
  listsRef.current = lists;

  function onDragStart(event: DragStartEvent) {
    const initial = { always: prefs.always.slice(), demand: prefs.demand.concat(offKeys) };
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
          id={LIST_ALWAYS}
          keys={lists.always}
          prefs={prefs}
          title="Always Visible"
          onToggleRow={onToggleRow}
        />
        <MetricSection
          byKey={byKey}
          id={LIST_DEMAND}
          keys={lists.demand}
          prefs={prefs}
          title="On Demand"
          onToggleRow={onToggleRow}
        />
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
  id: string;
  keys: string[];
  prefs: { off: Record<string, boolean> };
  title: string;
  onToggleRow: (key: string, on: boolean) => void;
}

function MetricSection({ byKey, id, keys, prefs, title, onToggleRow }: MetricSectionProps) {
  return (
    <div className="flex flex-col gap-[var(--header-card-gap)]">
      <div className="section-title">{title}</div>
      <SortableColumn id={id} items={keys} className="card-surface">
        {keys.length === 0 ? <div className="drop-zone">Drag metrics here</div> : null}
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
  onToggle?: (on: boolean) => void;
}

/** CustomizeMetricRow: grip, metric title, on/off switch. */
function MetricTuneRow({ enabled, handle, row, onToggle }: MetricTuneRowProps) {
  const title = String(row.label || row.kind);
  return (
    <div data-row-key={rowKey(row)} className="flex items-center gap-[10px] px-3 py-[var(--pad-control)]">
      <DragHandle attributes={handle?.attributes} listeners={handle?.listeners} />
      <span className="min-w-0 flex-1 truncate">{title}</span>
      <Switch checked={enabled} aria-label={`Show ${title}`} onCheckedChange={(on) => onToggle?.(on === true)} />
    </div>
  );
}
