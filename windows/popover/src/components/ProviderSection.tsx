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
import { Hint } from "@/components/Hint";
import { ProviderIcon } from "@/components/ProviderIcon";
import { ResetPopover } from "@/components/ResetPopover";
import { RowMenu, type RowAction } from "@/components/RowMenu";
import { TruncatedText, clippedHint, useClipped } from "@/components/TruncatedText";
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
import { m } from "@/paraglide/messages.js";
import { translateUsage, useI18n } from "@/lib/i18n";
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
  paceWarmupHint,
  paceWarmupText,
  prefsForCard,
  providerIconId,
  resetAlternate,
  resetCreditDetails,
  resetText,
  rowKey,
  sendCommand,
  usageGoal,
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
  const { language } = useI18n();
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
        <UsageMetricRow
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

  // Collapsed, the expander closes the card: it takes over the card's bottom gutter so its hover
  // fill reaches the rounded bottom edge instead of stopping short of it.
  const expanderLast = showExpander && !expanded && !card.warning;

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
      <div
        className={cn(
          "pt-[var(--card-gutter)]",
          !expanderLast && "pb-[var(--card-gutter)]",
          lifted ? "lifted-surface" : "card-surface",
        )}
      >
        {card.errorTitle ? <ErrorRow explained={explainError(card.errorDetail, card.id, language)} /> : null}
        {alwaysRows.map((row, index) => renderRow(row, index, condensedAlways, false))}
        {showExpander ? (
          <button
            type="button"
            aria-expanded={expanded}
            aria-label={expanded ? m.show_less() : m.show_more()}
            className={cn(
              "hover-row flex justify-center pt-[var(--space-2xs)] text-label-2",
              expanderLast ? "pb-[calc(var(--space-2xs)+var(--card-gutter))]" : "pb-[var(--space-2xs)]",
            )}
            onClick={onToggleCollapse}
          >
            {expanded ? <MdiChevronUp className="size-[var(--icon-row)]" /> : <MdiChevronDown className="size-[var(--icon-row)]" />}
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
  const { language, metricLabel } = useI18n();
  const details = resetCreditDetails(row, nowMs, { timeFormat: layout.timeFormat, locale: language });
  const noun = row.available === 1 ? m.available_reset() : m.available_resets();
  return (
    <div
      className={cn(
        "flex items-center gap-[var(--row-gap)] px-[var(--card-pad)] pb-[var(--pad-text-row)]",
        condensedTop ? "pt-[var(--pad-text-row-condensed)]" : "pt-[var(--pad-text-row)]",
      )}
    >
      <span className={cn("shrink-0 font-semibold", demand ? "text-[length:var(--sz-demand)]" : "text-[length:var(--sz-label)]")}>{metricLabel(row.label)}</span>
      <span className="min-w-[var(--gap-controls)] flex-1" />
      <ResetPopover hidden={details.hidden} items={details.items}>
        <Chip aria-label={`${row.available} ${noun}; ${m.show_expiry_dates()}`} variant="compact">
          <span aria-hidden="true" className="size-[var(--dot-sm)] rounded-full bg-meter-yellow" />
          <span className="tabular-nums">{row.available} {row.available === 1 ? m.available_singular() : m.available()}</span>
        </Chip>
      </ResetPopover>
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
      className="group/header flex items-center gap-[var(--gap-inline)] px-[var(--card-pad)] py-[var(--space-2xs)]"
      {...handle?.attributes}
      {...handle?.listeners}
    >
      <ProviderIcon className="text-label-2" size="var(--sz-icon)" slug={providerIconId(card.id)} title={card.title} />
      <div className="flex min-w-0 items-baseline gap-[var(--gap-inline)]">
        <TruncatedText className="min-w-0 text-[length:var(--sz-header)] font-semibold">{card.title}</TruncatedText>
        {plan ? <span className="shrink-0 text-[length:var(--sz-badge)] text-label-2">{plan}</span> : null}
        {card.stale ? <span className="text-[length:var(--sz-badge)] text-label-3">{m.stale()}</span> : null}
      </div>
      {card.errorTitle ? (
        <MdiAlert className="size-[var(--icon-mark)] shrink-0 text-meter-red" aria-label={card.errorTitle}>
          <title>{card.errorDetail}</title>
        </MdiAlert>
      ) : card.warning ? (
        <MdiAlert className="size-[var(--icon-mark)] shrink-0 text-notice" aria-label={card.warning.title}>
          <title>{card.warning.raw}</title>
        </MdiAlert>
      ) : null}
      <span className="min-w-[var(--gap-controls)] flex-1" />
      {account ? <AccountControl account={account} title={card.title} onSwitch={onSwitchAccount} /> : null}
      {onCustomize ? (
        <HeaderAction icon={<MdiTune />} label={`${m.customize()} ${card.title}`} onClick={onCustomize} />
      ) : null}
      {onReset ? (
        <HeaderAction icon={<MdiRestore />} label={`${m.reset()} ${card.title}`} onClick={onReset} />
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
    <Hint content={label}>
      <button
        type="button"
        aria-label={label}
        className={cn("header-action [&_svg]:size-[var(--icon-row)]", className)}
        onClick={onClick}
        onKeyDown={(event) => event.stopPropagation()}
        onPointerDown={(event) => event.stopPropagation()}
      >
        {icon}
      </button>
    </Hint>
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
function UsageMetricRow({ demand, layout, nowMs, onToggleShowAs, row }: MetricRowProps) {
  const { language, metricLabel } = useI18n();
  // The fill follows the headline's reading (WidgetData.fraction): remaining in Left mode,
  // consumed in Used mode. The color is a verdict and never flips with the toggle.
  const fill = layout.showAs === "used" ? row.usedPercent : row.leftPercent;
  const spent = row.leftPercent === 0;
  const headline = translateUsage(language, headlineLabel(row, layout.showAs));
  const headlineAlt = translateUsage(language, headlineAlternate(row, layout.showAs));
  // The headline sits inside the Used/Left button, whose hint also carries the clipped text.
  const headlineText = useClipped<HTMLSpanElement>(headline);
  const resetOpts = { timeFormat: layout.timeFormat, locale: language };
  const reset = resetText(row, layout.resetTimes, nowMs, resetOpts);
  const resetHint = resetAlternate(row, layout.resetTimes, nowMs, resetOpts);
  const rowPace = pace(row, nowMs);
  const showPace = rowPace !== null && paceVisible(rowPace, layout);
  const paceNote = showPace && rowPace ? paceText(rowPace, nowMs, { resetTimes: layout.resetTimes, timeFormat: layout.timeFormat, locale: language }) : "";
  // Too early in the window for a projection: say so instead of leaving the note empty, when
  // the layout asks for pacing on every metric.
  const warmup = rowPace === null && layout.alwaysShowPace ? paceWarmupText(row, nowMs, language) : "";
  const behind = rowPace?.state === "behind";
  const tick = paceTickPercent(rowPace, layout.showAs);
  const goal = layout.usageGoal ? usageGoal(row, nowMs) : null;
  const goalLabel = goal ? (goal.estimated ? m.estimated_goal_now() : m.goal_now()) : "";
  return (
    <div className="flex flex-col gap-[var(--row-inner)] px-[var(--card-pad)] py-[var(--pad-bar-row)]">
      <div className="flex items-center gap-[var(--gap-item)]">
        <TruncatedText className={cn("font-semibold", demand ? "text-[length:var(--sz-demand)]" : "text-[length:var(--sz-label)]")}>{metricLabel(row.label)}</TruncatedText>
        {spent ? (
          <span className="ml-auto flex shrink-0 items-center gap-[var(--gap-inline)] text-[length:var(--sz-support)] text-label-2">
            <MdiFire className="size-[var(--icon-note)] text-meter-red" />
            {m.limit_reached()}
          </span>
        ) : showPace && rowPace && (paceNote !== "" || behind) ? (
          <Hint
            align="end"
            content={m.pace_by_reset({ percent: Math.round(rowPace.projectedPercent) })}
          >
            <span className="ml-auto flex shrink-0 items-center gap-[var(--gap-inline)] text-[length:var(--sz-support)] text-label-2">
              {behind ? <MdiFire className="size-[var(--icon-note)] text-meter-red" /> : null}
              {paceNote}
            </span>
          </Hint>
        ) : warmup ? (
          <Hint align="end" content={paceWarmupHint(row, language)}>
            <span className="ml-auto shrink-0 text-[length:var(--sz-support)] text-label-2">{warmup}</span>
          </Hint>
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
      <div className="flex items-baseline gap-[var(--gap-controls)] text-[length:var(--sz-support)] tabular-nums">
        <Hint align="start" content={clippedHint(headline, headlineText.clipped, headlineAlt)}>
          <button type="button" className="plain-btn hover-fill min-w-0" onClick={onToggleShowAs} onPointerEnter={headlineText.measure}>
            <span ref={headlineText.ref} className="block truncate">{headline}</span>
          </button>
        </Hint>
        <span className="min-w-[var(--gap-controls)] flex-1" />
        {reset ? (
          <TruncatedText align="end" className="text-label-2" hint={resetHint}>{reset}</TruncatedText>
        ) : null}
      </div>
      {goal ? (
        <div className="usage-goal mt-[var(--space-sm)]">
          <div className="flex items-center justify-between gap-[var(--gap-controls)] text-[length:var(--sz-badge)] text-label-2 tabular-nums">
            <span>{goalLabel}</span>
            <strong className="font-semibold">{Math.round(goal.percent)}%</strong>
          </div>
          <div
            className="usage-goal-meter mt-[var(--space-2xs)]"
            role="progressbar"
            aria-label={`${metricLabel(row.label)}: ${goalLabel}`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(goal.percent)}
          >
            <span className="usage-goal-meter-fill" style={{ width: `${goal.percent}%` }} />
          </div>
        </div>
      ) : null}
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
  const { metricLabel } = useI18n();
  const lines = row.kind === "block" ? row.body : [row.value];
  return (
    <div
      className={cn(
        "flex items-start gap-[var(--row-gap)] px-[var(--card-pad)] pb-[var(--pad-text-row)]",
        condensedTop ? "pt-[var(--pad-text-row-condensed)]" : "pt-[var(--pad-text-row)]",
      )}
    >
      <span className={cn("shrink-0 font-semibold", demand ? "text-[length:var(--sz-demand)]" : "text-[length:var(--sz-label)]")}>{metricLabel(row.label)}</span>
      <span className="min-w-[var(--gap-controls)] flex-1" />
      <span className="flex min-w-0 max-w-full flex-col items-end gap-[var(--gap-tight)] text-right text-[length:var(--sz-support)] tabular-nums">
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
    <div className="flex flex-col gap-[var(--gap-stack)] px-[var(--card-pad)] py-[var(--pad-text-row)]">
      <span className="text-[length:var(--sz-support)] font-semibold">{explained.title || m.couldn_t_update()}</span>
      {explained.hint ? (
        <span className="text-[length:var(--sz-badge)] leading-[var(--leading-note)] text-label-2">{explained.hint}</span>
      ) : null}
      {explained.action ? (
        <button
          type="button"
          className="action-btn mt-[var(--gap-stack)] w-fit"
          onClick={() => sendCommand(explained.action?.cmd)}
        >
          {explained.action.label}
        </button>
      ) : null}
    </div>
  );
}

function ProviderLinks({ links }: { links: Array<{ label: string; url: string }> }) {
  const { metricLabel } = useI18n();
  return (
    <div className="flex gap-[var(--gap-controls)] px-[var(--card-pad)] py-[var(--pad-text-row)]">
      {links.map((link) => (
        <Chip key={link.url} variant="link" onClick={() => sendCommand("open-url", { url: link.url })}>
          <TruncatedText className="min-w-0">{metricLabel(link.label)}</TruncatedText>
          <MdiArrowTopRight className="size-[var(--icon-mark)] shrink-0 text-label-2" />
        </Chip>
      ))}
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
    <Hint align="start" content={warning.raw}>
      <div className="mt-[var(--space-2xs)] flex items-start gap-[var(--gap-item)] border-t border-border px-[var(--card-pad)] pt-[var(--space-sm)] pb-[var(--space-2xs)] text-[length:var(--sz-badge)] leading-[var(--leading-note)] text-label-2">
        <MdiAlert className="mt-[var(--space-px)] size-[var(--icon-dismiss)] shrink-0 text-notice" />
        <span>
          {warning.title}
          {warning.hint ? ` · ${warning.hint}` : ""}
        </span>
      </div>
    </Hint>
  );
}
