import {
  closestCorners,
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  useDroppable,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragOverEvent,
  type DragStartEvent,
  type UniqueIdentifier,
} from "@dnd-kit/core";
import { restrictToVerticalAxis } from "@dnd-kit/modifiers";
import {
  arrayMove,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { type CSSProperties, type ReactNode, useState } from "react";
import { cn } from "@/lib/utils";

export function useTraySensors() {
  return useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
}

export function SortableItem({
  id,
  className,
  children,
}: {
  id: UniqueIdentifier;
  className?: string;
  children: (opts: {
    attributes: ReturnType<typeof useSortable>["attributes"];
    listeners: ReturnType<typeof useSortable>["listeners"];
    isDragging: boolean;
  }) => ReactNode;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
  const style: CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.45 : undefined,
    position: "relative",
    zIndex: isDragging ? 1 : undefined,
  };
  return (
    <div ref={setNodeRef} style={style} className={className}>
      {children({ attributes, listeners, isDragging })}
    </div>
  );
}

export function SortableColumn({
  id,
  items,
  className,
  children,
}: {
  id: UniqueIdentifier;
  items: UniqueIdentifier[];
  className?: string;
  children: ReactNode;
}) {
  const { setNodeRef, isOver } = useDroppable({ id });
  return (
    <SortableContext items={items} strategy={verticalListSortingStrategy}>
      <div
        ref={setNodeRef}
        className={cn(className, isOver && "ring-2 ring-ring/35", items.length === 0 && "min-h-10")}
      >
        {children}
      </div>
    </SortableContext>
  );
}

export function VerticalDnd({
  items,
  onReorder,
  overlay,
  children,
}: {
  items: string[];
  onReorder: (ids: string[]) => void;
  overlay?: (activeId: string) => ReactNode;
  children: ReactNode;
}) {
  const sensors = useTraySensors();
  const [activeId, setActiveId] = useState<string | null>(null);

  function onDragStart(event: DragStartEvent) {
    setActiveId(String(event.active.id));
  }

  function onDragEnd(event: DragEndEvent) {
    const current = activeId;
    setActiveId(null);
    const overId = event.over?.id;
    if (!overId || !current || current === String(overId)) return;
    const oldIndex = items.indexOf(current);
    const newIndex = items.indexOf(String(overId));
    if (oldIndex < 0 || newIndex < 0) return;
    onReorder(arrayMove(items, oldIndex, newIndex));
  }

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCorners}
      modifiers={[restrictToVerticalAxis]}
      onDragStart={onDragStart}
      onDragEnd={onDragEnd}
      onDragCancel={() => setActiveId(null)}
    >
      <SortableContext items={items} strategy={verticalListSortingStrategy}>
        {children}
      </SortableContext>
      <DragOverlay dropAnimation={null}>{activeId && overlay ? overlay(activeId) : null}</DragOverlay>
    </DndContext>
  );
}

export const LIST_ALWAYS = "list:always";
export const LIST_DEMAND = "list:demand";

export type RowLists = { always: string[]; demand: string[] };

export function findRowList(id: UniqueIdentifier, lists: RowLists): keyof RowLists | null {
  const value = String(id);
  if (value === LIST_ALWAYS || lists.always.includes(value)) return "always";
  if (value === LIST_DEMAND || lists.demand.includes(value)) return "demand";
  return null;
}

export function handleRowDragOver(event: DragOverEvent, lists: RowLists): RowLists | null {
  if (!event.over) return null;
  const activeId = String(event.active.id);
  const overId = String(event.over.id);
  const from = findRowList(activeId, lists);
  const to = findRowList(overId, lists);
  if (!from || !to || from === to) return null;
  const next: RowLists = { always: lists.always.slice(), demand: lists.demand.slice() };
  const fromIndex = next[from].indexOf(activeId);
  if (fromIndex < 0) return null;
  next[from].splice(fromIndex, 1);
  const overIndex = overId === LIST_ALWAYS || overId === LIST_DEMAND ? next[to].length : next[to].indexOf(overId);
  next[to].splice(overIndex < 0 ? next[to].length : overIndex, 0, activeId);
  return next;
}

export function handleRowDragEnd(event: DragEndEvent, lists: RowLists): RowLists {
  if (!event.over) return lists;
  const activeId = String(event.active.id);
  const overId = String(event.over.id);
  const from = findRowList(activeId, lists);
  const to = findRowList(overId, lists);
  if (!from || !to) return lists;
  if (from !== to) {
    return handleRowDragOver(event, lists) || lists;
  }
  const oldIndex = lists[from].indexOf(activeId);
  const newIndex = overId === LIST_ALWAYS || overId === LIST_DEMAND ? lists[from].length - 1 : lists[from].indexOf(overId);
  if (oldIndex < 0 || newIndex < 0 || oldIndex === newIndex) return lists;
  return { ...lists, [from]: arrayMove(lists[from], oldIndex, newIndex) };
}
