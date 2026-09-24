import type { DraggableAttributes, DraggableSyntheticListeners } from "@dnd-kit/core";
import type { ReactNode } from "react";
import MdiAlert from "~icons/mdi/alert";
import MdiChevronDown from "~icons/mdi/chevron-down";
import MdiChevronUp from "~icons/mdi/chevron-up";
import MdiFire from "~icons/mdi/fire";
import MdiLoading from "~icons/mdi/loading";
import MdiRestore from "~icons/mdi/restore";
import MdiStar from "~icons/mdi/star";
import MdiStarOutline from "~icons/mdi/star-outline";
import MdiTune from "~icons/mdi/tune-variant";
import MdiArrowTopRight from "~icons/mdi/arrow-top-right";
import { Chip } from "@/components/Chip";
import { ProviderIcon } from "@/components/ProviderIcon";
import { ResetPopover } from "@/components/ResetPopover";
import { RowMenu, type RowAction } from "@/components/RowMenu";
import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type {
  BlockRow,
  Card,
  CardAccount,
  CardWarning,
  ExplainedError,
  Layout,
  MetricRow as MetricRowData,
  ResetCreditsRow as ResetCreditsRowData,
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
  isStarred,
  meterColor,
  providerLinks,
  pace,
  paceText,
  paceTickPercent,
  paceVisible,
  prefsForCard,
  providerIconId,
  resetCreditDetails,
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
  /** Switch control for an account card; absent on every other card. */
  account?: CardAccount | null;
  card: Card;
  handle?: SectionHandle;
  layout: Layout;
  lifted?: boolean;
  nowMs: number;
  onCustomize?: () => void;
  onReset?: () => void;
  onRowAction?: (key: string, action: RowAction) => void;
  onRowMenuOpenChange?: (open: boolean) => void;
  onSwitchAccount?: () => void;
  onToggleCollapse?: () => void;
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
  account,
  card,
  handle,
  layout,
  lifted,
  nowMs,
  onCustomize,
  onReset,
  onRowAction,
  onRowMenuOpenChange,
  onSwitchAccount,
  onToggleCollapse,
  onToggleShowAs,
}: ProviderSectionProps) {
  const prefs = prefsForCard(card, layout);
  const expanded = layout.collapsed?.[card.id] !== true;
  const opts = { hideExtras: layout.hideExtras, prefs };
  const alwaysRows: Row[] = visibleRowsFor(card, { ...opts, collapsed: true });
  const allRows: Row[] = visibleRowsFor(card, { ...opts, collapsed: false });
  const demandRows = allRows.slice(alwaysRows.length);
  const hasExtras = cardHasExtras(card, layout.hideExtras, prefs);
  const links = lifted ? [] : providerLinks(card.id);
  const showExpander = hasExtras || links.length > 0;
  const condensedAlways = new Set<number>(condensedTextRowIndexes(alwaysRows));
  const condensedDemand = new Set<number>(condensedTextRowIndexes(demandRows));

  function renderRow(row: Row, index: number, condensed: Set<number>, demand = false) {
    const key = rowKey(row);
    const node =
      row.kind === "metric" ? (
        <MetricRow
          key={key}
          demand={demand}
          layout={layout}
          nowMs={nowMs}
          row={row}
          onToggleShowAs={onToggleShowAs}
        />
      ) : row.kind === "resetCredits" ? (
        <ResetCreditsRow key={key} condensedTop={condensed.has(index)} demand={demand} layout={layout} nowMs={nowMs} row={row} />
      ) : (
        <TextRow key={key} condensedTop={condensed.has(index)} demand={demand} row={row} />
      );
    if (!onRowAction || lifted) return node;
    return (
      <RowMenu
        key={key}
        inAlways={prefs.always.includes(key)}
        providerTitle={card.title}
        starred={isStarred(layout.stars, card.id, key)}
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
      <ProviderSectionHeader
        account={account}
        card={card}
        handle={handle}
        onCustomize={onCustomize}
        onReset={onReset}
        onSwitchAccount={onSwitchAccount}
      />
      <div className={cn("py-[var(--card-gutter)]", lifted ? "lifted-surface" : "card-surface")}>
        {card.errorTitle ? <ErrorRow explained={explainError(card.errorDetail, card.id)} /> : null}
        {alwaysRows.map((row, index) => renderRow(row, index, condensedAlways, false))}
        {showExpander ? (
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
        {expanded ? demandRows.map((row, index) => renderRow(row, index, condensedDemand, true)) : null}
        {expanded && links.length ? <ProviderLinks links={links} /> : null}
        {card.warning ? <WarningStrip warning={card.warning} /> : null}
      </div>
    </section>
  );
}

interface ResetCreditsRowProps {
  condensedTop: boolean;
  demand?: boolean;
  layout: Layout;
  nowMs: number;
  row: ResetCreditsRowData;
}

function ResetCreditsRow({ condensedTop, demand, layout, nowMs, row }: ResetCreditsRowProps) {
  const details = resetCreditDetails(row, nowMs, { timeFormat: layout.timeFormat });
  const noun = row.available === 1 ? "available reset" : "available resets";
  return (
    <div
      className={cn(
        "flex items-center gap-[10px] px-[var(--card-pad)] pb-[var(--pad-text-row)]",
        condensedTop ? "pt-[var(--pad-text-row-condensed)]" : "pt-[var(--pad-text-row)]",
      )}
    >
      <span className={cn("shrink-0 font-semibold", demand ? "text-[length:var(--sz-demand)]" : "text-[length:var(--sz-label)]")}>{row.label}</span>
      <span className="min-w-3 flex-1" />
      <Tooltip>
        <TooltipTrigger asChild>
          <Badge asChild variant="secondary">
            <button type="button" aria-label={`${row.available} ${noun}; show expiry dates`}>
              <span aria-hidden="true" className="size-2 rounded-full bg-meter-yellow" />
              <span className="tabular-nums">{row.available} available</span>
            </button>
          </Badge>
        </TooltipTrigger>
        <TooltipContent
          align="end"
          collisionPadding={12}
          side="top"
          sideOffset={9}
          className="flex w-[min(330px,calc(100vw-24px))] flex-col gap-3 rounded-xl p-4 text-[length:var(--sz-support)]"
        >
          {details.items.map((item: { date: string; remaining: string; title: string }, index: number) => (
            <div key={`${item.date}-${index}`} className="flex items-center gap-3 tabular-nums" title={item.title || undefined}>
              <Badge variant={index === 0 ? "warning" : "default"} className="size-6 rounded-full p-0 text-[11px]">
                {index + 1}
              </Badge>
              <span className="min-w-0 flex-1 truncate font-medium">{item.date}</span>
              <span className="shrink-0 text-label-2">{item.remaining}</span>
            </div>
          ))}
          {details.hidden > 0 ? (
            <div className="text-right text-label-2">+{details.hidden} more</div>
          ) : null}
        </TooltipContent>
      </Tooltip>
    </div>
  );
}

interface ProviderSectionHeaderProps {
  account?: CardAccount | null;
  card: Card;
  handle?: SectionHandle;
  onCustomize?: () => void;
  onReset?: () => void;
  onSwitchAccount?: () => void;
}

/**
 * ProviderSectionHeader: gray provider mark, name, plan badge, stale hint, warning triangle,
 * and on the trailing edge the per-provider shortcuts OpenUsage keeps in the context menu:
 * Customize (this provider's rows) and Reset (its default rows). An account card also gets the
 * switch control first: a filled star on the login in use, an outline star button on the others. The
 * header is also the drag handle, so the buttons stop the pointer-down from starting a drag.
 */
export function ProviderSectionHeader({
  account,
  card,
  handle,
  onCustomize,
  onReset,
  onSwitchAccount,
}: ProviderSectionHeaderProps) {
  const plan = displayPlan(card.title, card.plan);
  return (
    <header
      className="group/header flex items-center gap-[5px] py-[2px] pr-1 pl-[2px]"
      {...handle?.attributes}
      {...handle?.listeners}
    >
      <ProviderIcon className="text-label-2" size="var(--sz-icon)" slug={providerIconId(card.id)} title={card.title} />
      <div className="flex min-w-0 items-baseline gap-[5px]">
        <span className="min-w-0 truncate text-[length:var(--sz-header)] font-semibold">{card.title}</span>
        {plan ? <span className="shrink-0 text-[length:var(--sz-badge)] text-label-2">{plan}</span> : null}
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
      {account ? <AccountControl account={account} title={card.title} onSwitch={onSwitchAccount} /> : null}
      {onCustomize ? (
        <HeaderAction icon={<MdiTune />} label={`Customize ${card.title}`} onClick={onCustomize} />
      ) : null}
      {onReset ? (
        <HeaderAction icon={<MdiRestore />} label={`Reset ${card.title}`} onClick={onReset} />
      ) : null}
    </header>
  );
}

interface AccountControlProps {
  account: CardAccount;
  title: string;
  onSwitch?: () => void;
}

/**
 * The account switch beside the header shortcuts, in the star language the row menu already
 * uses: the active login is a static filled star, not a button, since there is nothing to do
 * there; every other account is an outline star that makes it the active one. A running switch
 * spins in place; a failed one keeps the outline star, tinted red, with the reason as its tooltip.
 */
function AccountControl({ account, title, onSwitch }: AccountControlProps) {
  if (account.active) {
    const label = `${title} is the active account`;
    return (
      <span aria-label={label} className="header-action is-active [&_svg]:size-[14px]" role="img" title={label}>
        <MdiStar />
      </span>
    );
  }
  if (account.switching) {
    const label = `Switching to ${title}…`;
    return (
      <span aria-label={label} className="header-action [&_svg]:size-[14px]" role="status" title={label}>
        <MdiLoading className="animate-spin" />
      </span>
    );
  }
  if (!onSwitch || account.busy) return null;
  const label = account.error
    ? `Switch to ${title} failed: ${account.error}`
    : `Use ${title} (switches the CLI, desktop app and IDE extension)`;
  return (
    <HeaderAction
      className={account.error ? "is-failed" : undefined}
      icon={<MdiStarOutline />}
      label={label}
      onClick={onSwitch}
    />
  );
}

interface HeaderActionProps {
  className?: string;
  icon: ReactNode;
  label: string;
  onClick: () => void;
}

function HeaderAction({ className, icon, label, onClick }: HeaderActionProps) {
  return (
    <button
      type="button"
      aria-label={label}
      className={cn("header-action [&_svg]:size-[14px]", className)}
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
  demand?: boolean;
  layout: Layout;
  nowMs: number;
  onToggleShowAs?: () => void;
  row: MetricRowData;
}

/**
 * Bounded row: label (+ the pace note on the right: flame and "Limit in 3h" when behind, "~N%
 * spare" / "~N% left at reset" otherwise; "Limit reached" once spent) → capsule meter with the
 * pace tick where an even burn would sit → `52% left ⟷ Resets in 4d 17h`. The pace note and
 * tick show only off-pace unless Settings asks for them always (paceVisible).
 */
function MetricRow({ demand, layout, nowMs, onToggleShowAs, row }: MetricRowProps) {
  // The fill follows the headline's reading (WidgetData.fraction): remaining in Left mode,
  // consumed in Used mode. The color is a verdict and never flips with the toggle.
  const fill = layout.showAs === "used" ? row.usedPercent : row.leftPercent;
  const spent = row.leftPercent === 0;
  const headline = headlineLabel(row, layout.showAs);
  const headlineAlt = headlineAlternate(row, layout.showAs);
  const resetOpts = { timeFormat: layout.timeFormat };
  const reset = resetText(row, layout.resetTimes, nowMs, resetOpts);
  const rowPace = pace(row, nowMs);
  const showPace = rowPace !== null && paceVisible(rowPace, layout);
  const paceNote = showPace && rowPace ? paceText(rowPace, nowMs, { resetTimes: layout.resetTimes, timeFormat: layout.timeFormat }) : "";
  const behind = rowPace?.state === "behind";
  const tick = paceTickPercent(rowPace, layout.showAs);
  return (
    <div className="flex flex-col gap-[var(--row-inner)] px-[var(--card-pad)] py-[var(--pad-bar-row)]">
      <div className="flex items-center gap-[6px]">
        <span className={cn("truncate font-semibold", demand ? "text-[length:var(--sz-demand)]" : "text-[length:var(--sz-label)]")}>{row.label}</span>
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
            data-color={meterColor(row.severity, rowPace, spent)}
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
          <ResetPopover
            events={resetEvents(row)}
            nowMs={nowMs}
            timeFormat={layout.timeFormat}
          >
            <button type="button" className="plain-btn truncate text-label-2">
              {reset}
            </button>
          </ResetPopover>
        ) : null}
      </div>
    </div>
  );
}

interface TextRowProps {
  condensedTop: boolean;
  demand?: boolean;
  row: BlockRow | TextRowData;
}

/** Unbounded row: no bar. Label on the left, the value (or block lines) right-aligned. */
function TextRow({ condensedTop, demand, row }: TextRowProps) {
  const lines = row.kind === "block" ? row.body : [row.value];
  return (
    <div
      className={cn(
        "flex items-start gap-[10px] px-[var(--card-pad)] pb-[var(--pad-text-row)]",
        condensedTop ? "pt-[var(--pad-text-row-condensed)]" : "pt-[var(--pad-text-row)]",
      )}
    >
      <span className={cn("shrink-0 font-semibold", demand ? "text-[length:var(--sz-demand)]" : "text-[length:var(--sz-label)]")}>{row.label}</span>
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
    <div className="flex flex-col gap-[3px] px-[var(--card-pad)] py-[var(--pad-text-row)]">
      <span className="text-[length:var(--sz-support)] font-semibold">{explained.title || "Couldn't update"}</span>
      {explained.hint ? (
        <span className="text-[length:var(--sz-badge)] leading-[1.35] text-label-2">{explained.hint}</span>
      ) : null}
      {explained.action ? (
        <button
          type="button"
          className="mt-1 h-6 w-fit rounded-[var(--radius-sm)] bg-[var(--control-fill)] px-2.5 text-[length:var(--sz-support)] hover:bg-[var(--control-fill-hover)]"
          onClick={() => sendCommand(explained.action?.cmd)}
        >
          {explained.action.label}
        </button>
      ) : null}
    </div>
  );
}

function ProviderLinks({ links }: { links: Array<{ label: string; url: string }> }) {
  return (
    <div className="flex gap-2 px-[var(--card-pad)] py-[var(--pad-text-row)]">
      {links.map((link) => (
        <Chip key={link.url} variant="link" onClick={() => sendCommand("open-url", { url: link.url })}>
          <span className="truncate">{link.label}</span>
          <MdiArrowTopRight className="size-2.5 shrink-0 text-label-2" />
        </Chip>
      ))}
    </div>
  );
}

function resetEvents(row: MetricRowData): Array<{ atMs: number }> {
  const at = Date.parse(row.resetAt || "");
  return Number.isFinite(at) ? [{ atMs: at }] : [];
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
      className="mt-[2px] flex items-start gap-[6px] border-t border-border px-[var(--card-pad)] pt-[7px] pb-[3px] text-[length:var(--sz-badge)] leading-[1.35] text-label-2"
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
