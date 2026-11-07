import type { DraggableAttributes, DraggableSyntheticListeners } from "@dnd-kit/core";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import MdiAlert from "~icons/mdi/alert";
import MdiChevronDown from "~icons/mdi/chevron-down";
import MdiChevronUp from "~icons/mdi/chevron-up";
import MdiContentCopy from "~icons/mdi/content-copy";
import MdiFire from "~icons/mdi/fire";
import MdiLoading from "~icons/mdi/loading";
import MdiStar from "~icons/mdi/star";
import MdiStarHalfFull from "~icons/mdi/star-half-full";
import MdiStarOutline from "~icons/mdi/star-outline";
import MdiTune from "~icons/mdi/tune-variant";
import MdiArrowTopRight from "~icons/mdi/arrow-top-right";
import MdiClockStart from "~icons/mdi/clock-start";
import MdiCheck from "~icons/mdi/check";
import { Chip } from "@/components/Chip";
import { IconButton } from "@/components/IconButton";
import { Hint } from "@/components/Hint";
import { AccountTabs } from "@/components/AccountTabs";
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
import { handFocusOnRemoval, usePopoverActions } from "@/lib/popoverActions";
import {
  cardHasExtras,
  condensedTextRowIndexes,
  displayPlan,
  desktopPreparation,
  explainError,
  headlineAlternate,
  headlineLabel,
  isStarred,
  meterColor,
  providerLinks,
  pace,
  paceNote,
  paceVerdict,
  paceTickPercent,
  paceVisible,
  paceWarmupHint,
  paceWarmupText,
  prefsForCard,
  resetAlternate,
  resetCreditDetails,
  resetText,
  rowKey,
  sendCommand,
  usageGoal,
  usageGoalPercent,
  visibleRowsFor,
  formatAgo,
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
  /** The provider's accounts, when it has two or more: the header reads the provider's name
   * and the account tabs sit between the header and the card. */
  group?: {
    name: string;
    cards: Array<{ id: string; title: string }>;
    activeId: string;
    onSelect: (id: string) => void;
  };
  layout: Layout;
  lifted?: boolean;
  nowMs: number;
  onCustomize?: () => void;
  onCopyImage?: () => void;
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
  group,
  layout,
  lifted,
  nowMs,
  onCustomize,
  onCopyImage,
  onRowAction,
  onRowMenuOpenChange,
  onSwitchAccount,
  onToggleCollapse,
  onToggleShowAs,
}: ProviderSectionProps) {
  const { language } = useI18n();
  const actions = usePopoverActions();
  // A provider with several accounts titles its card after itself; the single-account
  // screen — including the per-account desktop title — reads the card's own title.
  const grouped = group && group.cards.length > 1;
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
        nowMs={nowMs}
        onCustomize={onCustomize}
        onCopyImage={onCopyImage}
        onSwitchAccount={onSwitchAccount}
        title={grouped ? group.name : undefined}
      />
      {grouped ? (
        <div className="px-[var(--card-pad)]">
          <AccountTabs cards={group.cards} providerName={group.name} selected={group.activeId} onSelect={group.onSelect} />
        </div>
      ) : null}
      <div
        className={cn(
          "pt-[var(--card-gutter)]",
          !expanderLast && "pb-[var(--card-gutter)]",
          lifted ? "lifted-surface" : "card-surface",
        )}
      >
        {card.errorTitle ? <ErrorRow explained={explainError(card.errorDetail, card.id, language, actions.deviceProvider(card.id))} /> : null}
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
          <span
            aria-hidden="true"
            className={cn("size-[var(--dot-sm)] rounded-full", row.available > 0 ? "bg-meter-yellow" : "bg-muted-foreground")}
          />
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
  nowMs?: number;
  /** The header's name; the card's own title when absent (a multi-account provider names itself). */
  title?: string;
  onCustomize?: () => void;
  onCopyImage?: () => void;
  onSwitchAccount?: () => void;
}

/**
 * ProviderSectionHeader: gray provider mark, name, plan badge, stale hint, warning triangle,
 * and on the trailing edge the per-provider shortcuts OpenUsage keeps in the context menu:
 * copy the card as an image (only on the header's hover or focus, like the tray's own reveal)
 * and Customize (this provider's rows). An account card also gets the
 * switch control first: a filled star on the login in use, an outline star button on the others. The
 * header is also the drag handle, so the buttons stop the pointer-down from starting a drag.
 */
export function ProviderSectionHeader({
  account,
  card,
  handle,
  nowMs = 0,
  title,
  onCustomize,
  onCopyImage,
  onSwitchAccount,
}: ProviderSectionHeaderProps) {
  const plan = displayPlan(card.title, card.plan);
  const name = title || card.title;
  const { language } = useI18n();
  const age = card.stale && card.fetchedAt && nowMs > card.fetchedAt ? ` · ${formatAgo(nowMs - card.fetchedAt, language)}` : "";
  return (
    <header
      className="group/header flex items-center gap-[var(--gap-inline)] px-[var(--card-pad)] py-[var(--space-2xs)]"
      {...handle?.attributes}
      {...handle?.listeners}
    >
      <ProviderIcon className="text-label-2" size="var(--sz-icon)" slug={card.icon} title={name} />
      <div className="flex min-w-0 items-baseline gap-[var(--gap-inline)]">
        <TruncatedText className="min-w-0 text-[length:var(--sz-header)] font-semibold">{name}</TruncatedText>
        {plan ? <span className="shrink-0 text-[length:var(--sz-badge)] text-label-2">{plan}</span> : null}
        {card.stale ? <span className="text-[length:var(--sz-badge)] text-label-3">{m.stale()}{age}</span> : null}
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
      {account ? <DesktopPrepareControl account={account} card={card} nowMs={nowMs} /> : null}
      {account ? <AccountControl account={account} title={name} onSwitch={onSwitchAccount} /> : null}
      {onCopyImage ? (
        <HeaderAction
          // Always shown, beside the customize action; the exported PNG drops it (data-exporting).
          icon={<MdiContentCopy />}
          label={m.copy_as_image()}
          onClick={onCopyImage}
        />
      ) : null}
      {onCustomize ? (
        <HeaderAction icon={<MdiTune />} label={`${m.customize()} ${name}`} onClick={onCustomize} />
      ) : null}
    </header>
  );
}

interface AccountControlProps {
  account: CardAccount;
  title: string;
  onSwitch?: () => void;
}

/** No account swap: a separate official CLI turn and server-confirmed window. */
function DesktopPrepareControl({ account, card, nowMs }: { account: CardAccount; card: Card; nowMs: number }) {
  const state = desktopPreparation(card, account, nowMs);
  if (!state) return null;
  return (
    <Hint content={state.hint}>
      <span>
        <button type="button" className={cn("desktop-account-control", account.prepareError && state.label === "Prepare" && "is-failed")}
          disabled={state.disabled} aria-label={`${state.label} Claude Desktop account ${account.label}`}
          onClick={() => sendCommand("prepare-account", { vendor: "claude-desktop", label: account.label })}
          onKeyDown={(event) => event.stopPropagation()} onPointerDown={(event) => event.stopPropagation()}>
          {account.preparing ? <MdiLoading className="animate-spin" /> : state.label === "Ready" ? <MdiCheck /> : <MdiClockStart />}
          {state.label}
        </button>
      </span>
    </Hint>
  );
}

/**
 * The account switch beside the header shortcuts, in the star language the row menu already
 * uses: the active login is a static filled star, not a button, since there is nothing to do
 * there; every other account is an outline star that makes it the active one. A running switch
 * spins in place; a failed one keeps the outline star, tinted red, with the reason as its tooltip.
 */
function AccountControl({ account, title, onSwitch }: AccountControlProps) {
  // A Desktop switch quits and reopens Claude.app: the first click arms it.
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const timer = setTimeout(() => setArmed(false), 4000);
    return () => clearTimeout(timer);
  }, [armed]);
  if (account.active) {
    const label = `${title} is the active account`;
    return (
      <span aria-label={label} className="icon-btn header-action is-active" role="img" title={label}>
        <MdiStar />
      </span>
    );
  }
  if (account.switching) {
    const label = `Switching to ${title}…`;
    return (
      <span aria-label={label} className="icon-btn header-action" role="status" title={label}>
        <MdiLoading className="animate-spin" />
      </span>
    );
  }
  if (!onSwitch || account.busy) return null;
  const label = account.error
    ? `Switch to ${title} failed: ${account.error}`
    : account.desktop && armed
      ? `Click again to switch the Claude Desktop app to ${title}; it quits and reopens`
      : account.desktop
        ? `Use ${title} in the Claude Desktop app (quits and reopens it)`
        : `Use ${title} (switches the CLI, desktop app and IDE extension)`;
  return (
    <HeaderAction
      className={account.error ? "is-failed" : undefined}
      icon={armed ? <MdiStarHalfFull /> : <MdiStarOutline />}
      label={label}
      onClick={() => {
        if (account.desktop && !armed) {
          setArmed(true);
          return;
        }
        setArmed(false);
        onSwitch();
      }}
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
    <IconButton
      label={label}
      className={cn("header-action", className)}
      onClick={onClick}
      onKeyDown={(event) => event.stopPropagation()}
      onPointerDown={(event) => event.stopPropagation()}
    >
      {icon}
    </IconButton>
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
  // The fill is always the consumed share — ink is consumption, like every usage
  // gauge. The Used/Left toggle is a reading of the headline number only, and the
  // color is a verdict; filling with what remains in Left mode painted SuperGrok's
  // "88% left" as an almost-full red bar that read as nearly spent.
  const fill = row.usedPercent;
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
  const note = showPace && rowPace ? paceNote(rowPace, row.leftPercent, nowMs, { resetTimes: layout.resetTimes, timeFormat: layout.timeFormat, locale: language }) : "";
  // Too early in the window for a projection: say so instead of leaving the note empty, when
  // the layout asks for pacing on every metric.
  const warmup = rowPace === null && layout.alwaysShowPace ? paceWarmupText(row, nowMs, language) : "";
  // The flame is for a row that runs out well before its reset, not for any row a hair over the line.
  const behind = paceVerdict(rowPace, row.leftPercent) === "critical";
  // The tick sits in the meter's space, which always shows the consumed share.
  const tick = paceTickPercent(rowPace);
  const goal = layout.usageGoal ? usageGoal(row, nowMs) : null;
  const goalLabel = goal ? (goal.estimated ? m.estimated_goal_now() : m.goal_now()) : "";
  // Like the tick, the goal follows the headline's reading so the two numbers compare directly.
  const goalPercent = usageGoalPercent(goal, layout.showAs) ?? 0;
  return (
    <div className="flex flex-col gap-[var(--row-inner)] px-[var(--card-pad)] py-[var(--pad-bar-row)]">
      <div className="flex items-center gap-[var(--gap-item)]">
        <TruncatedText className={cn("font-semibold", demand ? "text-[length:var(--sz-demand)]" : "text-[length:var(--sz-label)]")}>{metricLabel(row.label)}</TruncatedText>
        {spent ? (
          <span className="ml-auto flex shrink-0 items-center gap-[var(--gap-inline)] text-[length:var(--sz-support)] text-label-2">
            <MdiFire className="size-[var(--icon-note)] text-meter-red" />
            {m.limit_reached()}
          </span>
        ) : showPace && rowPace && (note !== "" || behind) ? (
          <Hint
            align="end"
            content={m.pace_by_reset({ percent: Math.round(rowPace.projectedPercent) })}
          >
            <span className="ml-auto flex shrink-0 items-center gap-[var(--gap-inline)] text-[length:var(--sz-support)] text-label-2">
              {behind ? <MdiFire className="size-[var(--icon-note)] text-meter-red" /> : null}
              {note}
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
            data-color={meterColor(row.leftPercent, rowPace, spent)}
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
            <strong className="font-semibold">{Math.round(goalPercent)}%</strong>
          </div>
          <div
            className="usage-goal-meter mt-[var(--space-2xs)]"
            role="progressbar"
            aria-label={`${metricLabel(row.label)}: ${goalLabel}`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(goalPercent)}
          >
            <span className="usage-goal-meter-fill" style={{ width: `${goalPercent}%` }} />
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
 * A provider with nothing to show: the verdict, the one-line hint, and — when there is a fix to
 * offer — a small action button. A missing key opens the key dialog, a device-code provider that
 * is not signed in opens the sign-in sheet, and no provider switched on opens Customize, all
 * inside the popover; the rest are host commands. Terminal-side fixes (the other sign-ins) and
 * waits (429) get no button.
 */
export function ErrorRow({ explained }: ErrorRowProps) {
  const actions = usePopoverActions();
  function run(trigger: HTMLElement) {
    const action = explained.action;
    if (!action) return;
    if (action.cmd === "add-key" && action.id) actions.addKey(action.id, trigger);
    else if (action.cmd === "sign-in" && action.id) actions.signIn(action.id, trigger);
    else if (action.cmd === "customize") actions.openCustomize();
    else sendCommand(action.cmd);
  }
  return (
    <div className="flex flex-col gap-[var(--gap-stack)] px-[var(--card-pad)] py-[var(--pad-text-row)]">
      <span className="text-[length:var(--sz-support)] font-semibold">{explained.title || m.couldn_t_update()}</span>
      {explained.hint ? (
        <span className="text-[length:var(--sz-badge)] leading-[var(--leading-note)] text-label-2">{explained.hint}</span>
      ) : null}
      {explained.action ? (
        <ErrorActionButton
          label={explained.action.label}
          providerId={explained.action.cmd === "add-key" || explained.action.cmd === "sign-in" ? explained.action.id : undefined}
          onClick={run}
        />
      ) : null}
    </div>
  );
}

interface ErrorActionButtonProps {
  label: string;
  /** Set for "Add key": the provider whose card this row stands in for. */
  providerId?: string;
  onClick: (trigger: HTMLElement) => void;
}

/**
 * The error row's one action. "Add key" goes away once the key is accepted,
 * often after the key dialog has handed focus back to it and the next report
 * arrives with the card's metrics: removed while focused, it passes focus to
 * the card's header action instead of leaving it on the body.
 */
function ErrorActionButton({ label, providerId, onClick }: ErrorActionButtonProps) {
  const button = useRef<HTMLButtonElement | null>(null);
  useLayoutEffect(() => {
    const node = button.current;
    // A layout cleanup on removal runs while the node is still in the document.
    return () => {
      if (providerId) handFocusOnRemoval(node, providerId);
    };
  }, [providerId]);
  return (
    <button
      ref={button}
      type="button"
      className="action-btn mt-[var(--gap-stack)] w-fit"
      data-key-row={providerId}
      onClick={(event) => onClick(event.currentTarget)}
    >
      {label}
    </button>
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
