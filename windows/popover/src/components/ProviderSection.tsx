import type { DraggableAttributes, DraggableSyntheticListeners } from "@dnd-kit/core";
import type { ReactNode } from "react";
import MdiAlert from "~icons/mdi/alert";
import MdiChevronDown from "~icons/mdi/chevron-down";
import MdiChevronUp from "~icons/mdi/chevron-up";
import MdiFire from "~icons/mdi/fire";
import MdiRestore from "~icons/mdi/restore";
import MdiTune from "~icons/mdi/tune-variant";
import { ProviderIcon } from "@/components/ProviderIcon";
import { RowMenu, type RowAction } from "@/components/RowMenu";
import type {
  BlockRow,
  Card,
  CardWarning,
  ExplainedError,
  Layout,
  MetricRow as MetricRowData,
  Row,
  TextRow as TextRowData,
} from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  cardHasExtras,
  condensedTextRowIndexes,
  displayPlan,
  explainError,
  headlineAlternate,
  headlineLabel,
  meterColor,
  pace,
  paceText,
  paceTickPercent,
  paceVisible,
  prefsForCard,
  providerIconId,
  resetAlternate,
  resetText,
  rowKey,
  sendCommand,
  visibleRowsFor,
} from "../model.js";

interface SectionHandle {
  attributes?: DraggableAttributes;
  listeners?: DraggableSyntheticListeners;
}

interface ProviderSectionProps {
  card: Card;
  handle?: SectionHandle;
  layout: Layout;
  lifted?: boolean;
  nowMs: number;
  onCustomize?: () => void;
  onReset?: () => void;
  onRowAction?: (key: string, action: RowAction) => void;
  onRowMenuOpenChange?: (open: boolean) => void;
  onToggleCollapse?: () => void;
  onToggleResetTimes?: () => void;
  onToggleShowAs?: () => void;
}

function noop() {}

/**
 * One provider on the dashboard: the section header outside the card, then the grouped metric
 * card (WidgetGroupedListView.section + DashboardMetricCard). Always Visible rows first, the
 * expand caret, then the On Demand rows while expanded. With `onRowAction` each row also gets
 * the right-click menu; the drag overlay (`lifted`) never does.
 */
export function ProviderSection({
  card,
  handle,
  layout,
  lifted,
  nowMs,
  onCustomize,
  onReset,
  onRowAction,
  onRowMenuOpenChange,
  onToggleCollapse,
  onToggleResetTimes,
  onToggleShowAs,
}: ProviderSectionProps) {
  const prefs = prefsForCard(card, layout);
  const expanded = layout.collapsed?.[card.id] !== true;
  const opts = { hideExtras: layout.hideExtras, prefs };
  const alwaysRows: Row[] = visibleRowsFor(card, { ...opts, collapsed: true });
  const allRows: Row[] = visibleRowsFor(card, { ...opts, collapsed: false });
  const demandRows = allRows.slice(alwaysRows.length);
  const hasExtras = cardHasExtras(card, layout.hideExtras, prefs);
  const condensedAlways = new Set<number>(condensedTextRowIndexes(alwaysRows));
  const condensedDemand = new Set<number>(condensedTextRowIndexes(demandRows));

  function renderRow(row: Row, index: number, condensed: Set<number>) {
    const key = rowKey(row);
    const node =
      row.kind === "metric" ? (
        <MetricRow
          key={key}
          layout={layout}
          nowMs={nowMs}
          row={row}
          onToggleResetTimes={onToggleResetTimes}
          onToggleShowAs={onToggleShowAs}
        />
      ) : (
        <TextRow key={key} condensedTop={condensed.has(index)} row={row} />
      );
    if (!onRowAction || lifted) return node;
    return (
      <RowMenu
        key={key}
        inAlways={prefs.always.includes(key)}
        providerTitle={card.title}
        onAction={(action) => onRowAction(key, action)}
        onOpenChange={onRowMenuOpenChange || noop}
      >
        {node}
      </RowMenu>
    );
  }

  return (
    <section
      data-card-id={card.id}
      className={cn("flex flex-col gap-[var(--header-card-gap)]", lifted && "rounded-[var(--card-radius)]")}
    >
      <ProviderSectionHeader card={card} handle={handle} onCustomize={onCustomize} onReset={onReset} />
      <div className={cn("py-[var(--card-gutter)]", lifted ? "lifted-surface" : "card-surface")}>
        {card.errorTitle ? <ErrorRow explained={explainError(card.errorDetail, card.id)} /> : null}
        {alwaysRows.map((row, index) => renderRow(row, index, condensedAlways))}
        {hasExtras ? (
          <button
            type="button"
            aria-expanded={expanded}
            aria-label={expanded ? "Show less" : "Show more"}
            className="plain-btn flex w-full justify-center py-[5px] text-label-2"
            onClick={onToggleCollapse}
          >
            {expanded ? <MdiChevronUp className="size-3.5" /> : <MdiChevronDown className="size-3.5" />}
          </button>
        ) : null}
        {expanded ? demandRows.map((row, index) => renderRow(row, index, condensedDemand)) : null}
        {card.warning ? <WarningStrip warning={card.warning} /> : null}
      </div>
    </section>
  );
}

interface ProviderSectionHeaderProps {
  card: Card;
  handle?: SectionHandle;
  onCustomize?: () => void;
  onReset?: () => void;
}

/**
 * ProviderSectionHeader: gray provider mark, name, plan badge, stale hint, warning triangle,
 * and on the trailing edge the per-provider shortcuts OpenUsage keeps in the context menu:
 * Customize (this provider's rows) and Reset (its default rows). The header is also the
 * drag handle, so the buttons stop the pointer-down from starting a drag.
 */
export function ProviderSectionHeader({ card, handle, onCustomize, onReset }: ProviderSectionHeaderProps) {
  const plan = displayPlan(card.title, card.plan);
  return (
    <header
      className="group/header flex items-center gap-[5px] py-[2px] pr-1 pl-[2px]"
      {...handle?.attributes}
      {...handle?.listeners}
    >
      <ProviderIcon className="text-label-2" size="var(--sz-icon)" slug={providerIconId(card.id)} title={card.title} />
      <div className="flex min-w-0 items-baseline gap-[5px]">
        <span className="truncate text-[length:var(--sz-header)] font-semibold">{card.title}</span>
        {plan ? <span className="truncate text-[length:var(--sz-badge)] text-label-2">{plan}</span> : null}
        {card.stale ? <span className="text-[length:var(--sz-badge)] text-label-3">stale</span> : null}
      </div>
      {card.errorTitle ? (
        <MdiAlert className="size-2.5 shrink-0 text-meter-red" aria-label={card.errorTitle}>
          <title>{card.errorDetail}</title>
        </MdiAlert>
      ) : card.warning ? (
        <MdiAlert className="size-2.5 shrink-0 text-notice" aria-label={card.warning.title}>
          <title>{card.warning.raw}</title>
        </MdiAlert>
      ) : null}
      <span className="min-w-2 flex-1" />
      {onCustomize ? (
        <HeaderAction icon={<MdiTune />} label={`Customize ${card.title}`} onClick={onCustomize} />
      ) : null}
      {onReset ? (
        <HeaderAction icon={<MdiRestore />} label={`Reset ${card.title}`} onClick={onReset} />
      ) : null}
    </header>
  );
}

interface HeaderActionProps {
  icon: ReactNode;
  label: string;
  onClick: () => void;
}

function HeaderAction({ icon, label, onClick }: HeaderActionProps) {
  return (
    <button
      type="button"
      aria-label={label}
      className="header-action [&_svg]:size-[14px]"
      title={label}
      onClick={onClick}
      onKeyDown={(event) => event.stopPropagation()}
      onPointerDown={(event) => event.stopPropagation()}
    >
      {icon}
    </button>
  );
}

interface MetricRowProps {
  layout: Layout;
  nowMs: number;
  onToggleResetTimes?: () => void;
  onToggleShowAs?: () => void;
  row: MetricRowData;
}

/**
 * Bounded row: label (+ the pace note on the right: flame and "Limit in 3h" when behind, "~N%
 * spare" / "~N% left at reset" otherwise; "Limit reached" once spent) → capsule meter with the
 * pace tick where an even burn would sit → `52% left ⟷ Resets in 4d 17h`. The pace note and
 * tick show only off-pace unless Settings asks for them always (paceVisible).
 */
function MetricRow({ layout, nowMs, onToggleResetTimes, onToggleShowAs, row }: MetricRowProps) {
  // The fill follows the headline's reading (WidgetData.fraction): remaining in Left mode,
  // consumed in Used mode. The color is a verdict and never flips with the toggle.
  const fill = layout.showAs === "used" ? row.usedPercent : row.leftPercent;
  const spent = row.leftPercent === 0;
  const headline = headlineLabel(row, layout.showAs);
  const headlineAlt = headlineAlternate(row, layout.showAs);
  const resetOpts = { timeFormat: layout.timeFormat };
  const reset = resetText(row, layout.resetTimes, nowMs, resetOpts);
  const resetAlt = resetAlternate(row, layout.resetTimes, nowMs, resetOpts);
  const rowPace = pace(row, nowMs);
  const showPace = rowPace !== null && paceVisible(rowPace, layout);
  const paceNote = showPace && rowPace ? paceText(rowPace, nowMs, { resetTimes: layout.resetTimes, timeFormat: layout.timeFormat }) : "";
  const behind = rowPace?.state === "behind";
  const tick = paceTickPercent(rowPace, layout.showAs);
  return (
    <div className="flex flex-col gap-[var(--row-inner)] px-[14px] py-[var(--pad-bar-row)]">
      <div className="flex items-center gap-[6px]">
        <span className="truncate text-[length:var(--sz-label)] font-semibold">{row.label}</span>
        {spent ? (
          <span className="ml-auto flex shrink-0 items-center gap-[3px] text-[length:var(--sz-support)] text-label-2">
            <MdiFire className="size-[11px] text-meter-red" />
            Limit reached
          </span>
        ) : showPace && rowPace && (paceNote !== "" || behind) ? (
          <span
            className="ml-auto flex shrink-0 items-center gap-[3px] text-[length:var(--sz-support)] text-label-2"
            title={`On this pace, ${Math.round(rowPace.projectedPercent)}% of the quota is used by the reset`}
          >
            {behind ? <MdiFire className="size-[11px] text-meter-red" /> : null}
            {paceNote}
          </span>
        ) : null}
      </div>
      <div className="meter-wrap">
        <div className="meter" aria-hidden="true">
          <div
            className="meter-fill"
            data-color={meterColor(row.severity)}
            data-empty={fill === 0 ? "true" : "false"}
            style={{ width: `${fill}%` }}
          />
        </div>
        {showPace && tick !== null ? (
          <span
            aria-hidden="true"
            className="meter-tick"
            style={{ left: `clamp(1px, ${tick}%, calc(100% - 1px))` }}
          />
        ) : null}
      </div>
      <div className="flex items-baseline gap-2 text-[length:var(--sz-support)] tabular-nums">
        <button
          type="button"
          className="plain-btn truncate"
          title={headlineAlt || undefined}
          onClick={onToggleShowAs}
        >
          {headline}
        </button>
        <span className="min-w-2 flex-1" />
        {reset ? (
          <button
            type="button"
            className="plain-btn truncate text-label-2"
            title={resetAlt || undefined}
            onClick={onToggleResetTimes}
          >
            {reset}
          </button>
        ) : null}
      </div>
    </div>
  );
}

interface TextRowProps {
  condensedTop: boolean;
  row: BlockRow | TextRowData;
}

/** Unbounded row: no bar. Label on the left, the value (or block lines) right-aligned. */
function TextRow({ condensedTop, row }: TextRowProps) {
  const lines = row.kind === "block" ? row.body : [row.value];
  return (
    <div
      className={cn(
        "flex items-start gap-[10px] px-[14px] pb-[var(--pad-text-row)]",
        condensedTop ? "pt-[var(--pad-text-row-condensed)]" : "pt-[var(--pad-text-row)]",
      )}
    >
      <span className="shrink-0 text-[length:var(--sz-support)] font-semibold">{row.label}</span>
      <span className="min-w-3 flex-1" />
      <span className="flex min-w-0 max-w-full flex-col items-end gap-[2px] text-right text-[length:var(--sz-support)] tabular-nums">
        {lines.map((line, index) => (
          <span key={index} className="max-w-full break-words [overflow-wrap:anywhere]">
            {line}
          </span>
        ))}
      </span>
    </div>
  );
}

interface ErrorRowProps {
  explained: ExplainedError;
}

/**
 * A provider with nothing to show: the verdict, the one-line hint, and — when the fix is a host
 * command — a small action button. Terminal-side fixes (sign-in) and waits (429) get no button.
 */
export function ErrorRow({ explained }: ErrorRowProps) {
  return (
    <div className="flex flex-col gap-[3px] px-[14px] py-[var(--pad-text-row)]">
      <span className="text-[length:var(--sz-support)] font-semibold">{explained.title || "Couldn't update"}</span>
      {explained.hint ? (
        <span className="text-[length:var(--sz-badge)] leading-[1.35] text-label-2">{explained.hint}</span>
      ) : null}
      {explained.action ? (
        <button
          type="button"
          className="mt-1 h-6 w-fit rounded-[6px] bg-[var(--control-fill)] px-2.5 text-[length:var(--sz-support)] hover:bg-[var(--control-fill-hover)]"
          onClick={() => sendCommand(explained.action?.cmd)}
        >
          {explained.action.label}
        </button>
      ) : null}
    </div>
  );
}

interface WarningStripProps {
  warning: CardWarning;
}

/**
 * The card's cached-data note: an orange mark and one quiet line, under a hairline at the bottom
 * of the card. The numbers above are the last good snapshot; the raw diagnosis lives in the hover.
 */
function WarningStrip({ warning }: WarningStripProps) {
  return (
    <div
      className="mt-[2px] flex items-start gap-[6px] border-t border-border px-[14px] pt-[7px] pb-[3px] text-[length:var(--sz-badge)] leading-[1.35] text-label-2"
      title={warning.raw}
    >
      <MdiAlert className="mt-[1px] size-3 shrink-0 text-notice" />
      <span>
        {warning.title}
        {warning.hint ? ` · ${warning.hint}` : ""}
      </span>
    </div>
  );
}
