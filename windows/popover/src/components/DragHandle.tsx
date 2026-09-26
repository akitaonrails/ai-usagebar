import type { DraggableAttributes, DraggableSyntheticListeners } from "@dnd-kit/core";
import MdiMenu from "~icons/mdi/menu";
import { m } from "@/paraglide/messages.js";
import { cn } from "@/lib/utils";

interface DragHandleProps {
  attributes?: DraggableAttributes;
  className?: string;
  label?: string;
  listeners?: DraggableSyntheticListeners;
}

/** ReorderGrip: `line.3.horizontal`, 12pt semibold, tertiary, in a 16×22 hit box. */
export function DragHandle({ attributes, className, label, listeners }: DragHandleProps) {
  return (
    <button type="button" aria-label={label ?? m.reorder()} className={cn("grip", className)} {...attributes} {...listeners}>
      <MdiMenu className="size-[var(--icon-row)]" />
    </button>
  );
}
