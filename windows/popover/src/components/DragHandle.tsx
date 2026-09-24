import type { DraggableAttributes, DraggableSyntheticListeners } from "@dnd-kit/core";
import MdiMenu from "~icons/mdi/menu";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";

interface DragHandleProps {
  attributes?: DraggableAttributes;
  className?: string;
  label?: string;
  listeners?: DraggableSyntheticListeners;
}

/** ReorderGrip: `line.3.horizontal`, 12pt semibold, tertiary, in a 16×22 hit box. */
export function DragHandle({ attributes, className, label = "Reorder", listeners }: DragHandleProps) {
  const { t } = useI18n();
  return (
    <button type="button" aria-label={t(label)} className={cn("grip", className)} {...attributes} {...listeners}>
      <MdiMenu className="size-3.5" />
    </button>
  );
}
