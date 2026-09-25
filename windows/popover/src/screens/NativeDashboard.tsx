import { useState } from "react";
import MdiCogOutline from "~icons/mdi/cog-outline";
import MdiRefresh from "~icons/mdi/refresh";
import { Hint } from "@/components/Hint";
import { ProviderIcon } from "@/components/ProviderIcon";
import { ProviderSection } from "@/components/ProviderSection";
import type { RowAction } from "@/components/RowMenu";
import { accountSwitchFor, nextUpdateLabel, sendCommand } from "../model.js";
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
  onResetProvider: (id: string) => void;
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

function providerPreview(card: Card): string {
  const metric = primaryMetric(card);
  if (metric) return metric.headline === "value" ? metric.value : `${metric.usedPercent}%`;
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
  onResetProvider,
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
              const preview = providerPreview(card);
              // Logo and value only, to fit more tabs: the name is in the hint, the label and the card below.
              return (
                <Hint key={card.id} content={card.title}>
                  <button
                    type="button"
                    aria-label={preview ? `${card.title} ${preview}` : card.title}
                    aria-pressed={active}
                    className="native-tab"
                    data-active={active}
                    onClick={() => setSelectedId(card.id)}
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
              onReset={() => onResetProvider(selected.id)}
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
