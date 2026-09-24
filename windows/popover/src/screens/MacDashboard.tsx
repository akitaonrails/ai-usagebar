import { useState } from "react";
import MdiCogOutline from "~icons/mdi/cog-outline";
import MdiRefresh from "~icons/mdi/refresh";
import { ProviderIcon } from "@/components/ProviderIcon";
import { useI18n } from "@/lib/i18n";
import type { Card, MetricRow, Payload, Row } from "@/lib/types";
import { nextUpdateLabel, resetText, sendCommand } from "../model.js";

interface MacDashboardProps {
  cards: Card[];
  nowMs: number;
  payload: Payload;
  onOpenCustomize: () => void;
  onOpenSettings: () => void;
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

function Metric({ row, nowMs }: { row: MetricRow; nowMs: number }) {
  const { language, metricLabel, t } = useI18n();
  const percent = Math.min(100, Math.max(0, Number(row.usedPercent) || 0));
  const reset = resetText(row, "countdown", nowMs, { locale: language });
  const balance = row.headline === "value";
  const label = row.label === "Session" ? `${t("Session")} (5h)` : metricLabel(row.label);
  return (
    <div className="mac-metric">
      <div className="mac-metric-heading">
        <span>{label}</span>
        <strong>{balance ? row.value : `${percent}%`}</strong>
      </div>
      <div
        className="mac-meter"
        role="progressbar"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >
        <span className="mac-meter-fill" data-severity={row.severity} style={{ width: `${percent}%` }} />
      </div>
      {reset || row.detail || (balance && percent > 0) ? (
        <div className="mac-metric-note">
          <span>{reset || row.detail}</span>
          {balance && percent > 0 ? <span>{percent}% {t("used")}</span> : null}
        </div>
      ) : null}
    </div>
  );
}

function DetailRow({ row, nowMs }: { row: Row; nowMs: number }) {
  const { metricLabel, t } = useI18n();
  if (row.kind === "metric") return <Metric row={row} nowMs={nowMs} />;
  if (row.kind === "text") {
    return (
      <div className="mac-detail-row">
        <span>{metricLabel(row.label)}</span>
        <strong>{row.value}</strong>
      </div>
    );
  }
  if (row.kind === "resetCredits") {
    return (
      <div className="mac-detail-row">
        <span>{metricLabel(row.label)}</span>
        <strong>{row.available} {t(row.available === 1 ? "available singular" : "available")}</strong>
      </div>
    );
  }
  return (
    <div className="mac-detail-block">
      <strong>{metricLabel(row.label)}</strong>
      {row.body.map((line, index) => <span key={`${index}:${line}`}>{line}</span>)}
    </div>
  );
}

/** A compact provider switcher for the macOS menu bar popover. */
export function MacDashboard({ cards, nowMs, payload, onOpenCustomize, onOpenSettings }: MacDashboardProps) {
  const { language, t } = useI18n();
  const [selectedId, setSelectedId] = useState("");
  const selected = cards.find((card) => card.id === selectedId)
    ?? cards.find((card) => card.id === payload.primary)
    ?? cards.find((card) => primaryMetric(card))
    ?? cards[0];
  const selectedEntry = payload.entries.find((entry) => entry.id === selected?.id);
  const updated = payload.generatedAt > 0
    ? Math.max(0, Math.floor((nowMs - payload.generatedAt) / 60_000))
    : null;

  return (
    <div className="mac-dashboard">
      <header className="mac-dashboard-header">
        <div>
          <h1>AI Usage</h1>
          <p>{t("Usage and balance")}</p>
        </div>
        <div className="mac-dashboard-actions">
          <button type="button" className="mac-icon-button" aria-label={t("Refresh")} title={t("Refresh")} onClick={() => sendCommand("refresh") }>
            <MdiRefresh aria-hidden />
          </button>
          <button type="button" className="mac-icon-button" aria-label={t("Settings")} title={t("Settings")} onClick={onOpenSettings}>
            <MdiCogOutline aria-hidden />
          </button>
        </div>
      </header>

      {cards.length ? (
        <>
          <div className="mac-provider-tabs" role="group" aria-label={t("Providers")}>
            {cards.map((card) => {
              const active = selected?.id === card.id;
              const name = payload.entries.find((entry) => entry.id === card.id)?.shortName || card.title;
              return (
                <button
                  key={card.id}
                  type="button"
                  aria-pressed={active}
                  className="mac-provider-tab"
                  data-active={active}
                  onClick={() => setSelectedId(card.id)}
                >
                  <ProviderIcon slug={card.id} title={card.title} size={17} />
                  <span className="mac-tab-name">{name}</span>
                  <span className="mac-tab-value">{providerPreview(card)}</span>
                </button>
              );
            })}
          </div>

          {selected ? (
            <section className="mac-provider-card" aria-label={selected.title}>
              <div className="mac-provider-heading">
                <span className="mac-provider-mark"><ProviderIcon slug={selected.id} title={selected.title} size={25} /></span>
                <span className="mac-provider-title">
                  <strong>{selected.title}</strong>
                  <small>{selected.plan || (selectedEntry?.status === "ready" ? t("Current usage") : t("Usage unavailable"))}{selected.stale ? ` · ${t("Cached")}` : ""}</small>
                </span>
                <button type="button" className="mac-provider-refresh" title={`${t("Refresh")} ${selected.title}`} aria-label={`${t("Refresh")} ${selected.title}`} onClick={() => sendCommand("refresh-entry", { id: selected.id })}>
                  <MdiRefresh aria-hidden />
                </button>
              </div>

              {selected.rows.length ? (
                <div className="mac-usage-section">
                  <div className="mac-section-label">{t("USAGE & BALANCE")}</div>
                  {selected.rows.map((row, index) => <DetailRow key={row.key || `${row.kind}:${index}`} row={row} nowMs={nowMs} />)}
                </div>
              ) : selected.error ? (
                <div className="mac-empty-state">
                  <strong>{t(selected.errorTitle || "Usage unavailable")}</strong>
                  <p>{t(selected.errorHint || selected.error)}</p>
                </div>
              ) : (
                <div className="mac-empty-state">{t("No usage or balance data yet.")}</div>
              )}
              {selected.error && selected.rows.length ? <p className="mac-cached-note">{t(selected.errorTitle)}: {t(selected.errorHint)}</p> : null}
            </section>
          ) : null}
        </>
      ) : (
        <div className="mac-empty-state">
          <strong>{payload.hostError ? t("Couldn't load usage") : payload.entries.length ? t("No providers shown") : t("No providers detected")}</strong>
          <p>{payload.hostError || (payload.entries.length ? t("Turn on a provider in Customize.") : t("Refresh to check your installed providers."))}</p>
          <button type="button" className="mac-retry-button" onClick={payload.entries.length ? onOpenCustomize : () => sendCommand("detect")}>
            {t(payload.entries.length ? "Customize Providers" : "Detect Providers")}
          </button>
        </div>
      )}

      <div className="mac-dashboard-status">
        <span>{updated === null ? t("Waiting for update") : updated < 1 ? t("Updated just now") : language === "pt-BR" ? `Atualizado há ${updated} min` : `Updated ${updated}m ago`}</span>
        <span>{nextUpdateLabel(payload, nowMs, language)}</span>
      </div>
    </div>
  );
}
