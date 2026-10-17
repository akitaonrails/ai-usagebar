import { useState } from "react";
import MdiCogOutline from "~icons/mdi/cog-outline";
import MdiRefresh from "~icons/mdi/refresh";
import { Hint } from "@/components/Hint";
import { ProviderIcon } from "@/components/ProviderIcon";
import { ProviderSection } from "@/components/ProviderSection";
import type { RowAction } from "@/components/RowMenu";
import { accountSwitchFor, hiddenMetricKeys, nextUpdateLabel, rowKey, sendCommand } from "../model.js";
import { DashboardBanners } from "./Dashboard";
import type { Card, Layout, MetricRow, Payload } from "@/lib/types";
import { m } from "@/paraglide/messages.js";
import { useI18n } from "@/lib/i18n";
import { wheelScrollRef } from "@/lib/wheelScroll";

interface NativeDashboardProps {
  cards: Card[];
  hint: boolean;
  layout: Layout;
  nowMs: number;
  payload: Payload;
  onCustomizeProvider: (id: string) => void;
  onDismissHint: () => void;
  onOpenCustomize: () => void;
  onOpenSettings: () => void;
  onRowAction: (providerId: string, rowKey: string, action: RowAction) => void;
  onRowMenuOpenChange: (open: boolean) => void;
  onSwitchAccount: (vendor: string, label: string) => void;
  onToggleCollapse: (id: string) => void;
  onToggleShowAs: () => void;
}

function primaryMetric(card: Card): MetricRow | undefined {
  return card.rows.find((row): row is MetricRow => row.kind === "metric" && row.headline === "percent")
    ?? card.rows.find((row): row is MetricRow => row.kind === "metric");
}

/**
 * The tab's number: the highest-percent quota window, like the Quattro bar's
 * default `auto` window (`maxPercent` in `omarchy/Model.js`), so a spent weekly
 * limit is not hidden behind an idle 5h session reading 0%. Grouped rows stand
 * in only when the card has no other percentage, and a per-model pool only
 * when nothing the provider shares is left: an exhausted Gemini 4 Argon is one
 * model's limit, not the provider's (#453). A metric hidden in Customize
 * never counts, so the tab cannot show a number the card below leaves out.
 */
function previewMetric(card: Card, layout: Layout): MetricRow | undefined {
  const hidden = new Set(hiddenMetricKeys(card, layout));
  const shown = { ...card, rows: card.rows.filter((row) => !hidden.has(rowKey(row))) };
  const percents = shown.rows.filter((row): row is MetricRow => row.kind === "metric" && row.headline === "percent");
  const shared = percents.filter((row) => !row.modelPool);
  const ranked = shared.length ? shared : percents;
  const windows = ranked.filter((row) => !row.grouped);
  const candidates = windows.length ? windows : ranked;
  if (!candidates.length) return primaryMetric(shown);
  return candidates.reduce((best, row) => (row.usedPercent > best.usedPercent ? row : best));
}

/** The tab's text in the layout's Used/Left reading, like the meters below it. */
function providerPreview(card: Card, layout: Layout): string {
  const metric = previewMetric(card, layout);
  if (metric) {
    if (metric.headline === "value") return metric.value;
    return `${layout.showAs === "used" ? metric.usedPercent : metric.leftPercent}%`;
  }
  if (card.error) return "—";
  const balance = card.rows.find((row) => row.kind === "text" && /balance|credit/i.test(row.label));
  return balance?.kind === "text" ? balance.value : "—";
}

/** Native dashboard tabs with the selected provider's shared dashboard section. */
export function NativeDashboard({
  cards,
  hint,
  layout,
  nowMs,
  payload,
  onCustomizeProvider,
  onDismissHint,
  onOpenCustomize,
  onOpenSettings,
  onRowAction,
  onRowMenuOpenChange,
  onSwitchAccount,
  onToggleCollapse,
  onToggleShowAs,
}: NativeDashboardProps) {
  const { language } = useI18n();
  const [selectedId, setSelectedId] = useState("");
  const selected = cards.find((card) => card.id === selectedId)
    ?? cards.find((card) => card.id === payload.primary)
    ?? cards.find((card) => primaryMetric(card))
    ?? cards[0];
  // The macOS menu bar's Quattro look draws the provider selected here, the way the
  // Quattro bar follows its selected entry. Only macOS has that look.
  function selectProvider(id: string) {
    setSelectedId(id);
    if (payload.os === "macos") sendCommand("select-provider", { id });
  }
  const selectedAccount = selected ? accountSwitchFor(selected.id, payload.accounts) : null;
  const updated = payload.generatedAt > 0
    ? Math.max(0, Math.floor((nowMs - payload.generatedAt) / 60_000))
    : null;

  return (
    <div className="native-dashboard">
      <header className="native-dashboard-header">
        <div>
          <h1>AI Usage</h1>
          <p>{m.usage_and_balance_subtitle()}</p>
        </div>
        <div className="native-dashboard-actions">
          <Hint content={m.refresh()}>
            <button type="button" className="native-icon-button" aria-label={m.refresh()} onClick={() => sendCommand("refresh")}>
              <MdiRefresh aria-hidden />
            </button>
          </Hint>
          <Hint content={m.settings()}>
            <button type="button" className="native-icon-button" aria-label={m.settings()} onClick={onOpenSettings}>
              <MdiCogOutline aria-hidden />
            </button>
          </Hint>
        </div>
      </header>

      <DashboardBanners
        compact
        hint={hint}
        payload={payload}
        onDismissHint={onDismissHint}
        onOpenCustomize={onOpenCustomize}
      />

      {cards.length ? (
        <>
          <div ref={wheelScrollRef} className="native-tabs native-provider-tabs" role="group" aria-label={m.providers()}>
            {cards.map((card) => {
              const active = selected?.id === card.id;
              const preview = providerPreview(card, layout);
              // Logo and value only, to fit more tabs: the name is in the hint, the label and the card below.
              return (
                <Hint key={card.id} content={card.title}>
                  <button
                    type="button"
                    aria-label={preview ? `${card.title} ${preview}` : card.title}
                    aria-pressed={active}
                    className="native-tab"
                    data-active={active}
                    onClick={() => selectProvider(card.id)}
                  >
                    <ProviderIcon className="text-label-2" slug={card.id} title={card.title} size={17} />
                    <span className="native-tab-value" data-text={preview}>{preview}</span>
                  </button>
                </Hint>
              );
            })}
          </div>

          {selected ? (
            <ProviderSection
              account={selectedAccount}
              card={selected}
              layout={layout}
              nowMs={nowMs}
              onCustomize={() => onCustomizeProvider(selected.id)}
              onRowAction={(key, action) => onRowAction(selected.id, key, action)}
              onRowMenuOpenChange={onRowMenuOpenChange}
              onSwitchAccount={() => {
                if (selectedAccount) onSwitchAccount(selectedAccount.vendor, selectedAccount.label);
              }}
              onToggleCollapse={() => onToggleCollapse(selected.id)}
              onToggleShowAs={onToggleShowAs}
            />
          ) : null}
        </>
      ) : (
        <div className="native-empty-state">
          <strong>{payload.hostError ? m.couldn_t_load_usage() : payload.entries.length ? m.no_providers_shown() : m.no_providers_detected()}</strong>
          <p>{payload.hostError || (payload.entries.length ? m.turn_on_a_provider_in_customize() : m.refresh_to_detect_hint())}</p>
          <button type="button" className="native-retry-button" onClick={payload.entries.length ? onOpenCustomize : () => sendCommand("detect")}>
            {payload.entries.length ? m.customize_providers() : m.detect_providers()}
          </button>
        </div>
      )}

      <div className="native-dashboard-status">
        <span>{updated === null ? m.waiting_for_update() : updated < 1 ? m.updated_just_now() : m.updated_minutes_ago({ minutes: updated })}</span>
        <span>{nextUpdateLabel(payload, nowMs, language)}</span>
      </div>
    </div>
  );
}
