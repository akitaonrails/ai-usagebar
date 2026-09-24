import assert from 'node:assert/strict';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

process.env.TZ = 'UTC';
const server = await createServer({ server: { middlewareMode: true, hmr: false, ws: false }, appType: 'custom' });

try {
  const { MacDashboard } = await server.ssrLoadModule('/src/screens/MacDashboard.tsx');
  const { Settings } = await server.ssrLoadModule('/src/screens/Settings.tsx');
  const { TooltipProvider } = await server.ssrLoadModule('/src/components/ui/tooltip.tsx');
  const { LanguageProvider } = await server.ssrLoadModule('/src/lib/i18n.tsx');
  const { emptyLayout, emptyPayload } = await server.ssrLoadModule('/src/model.js');
  const nowMs = Date.parse('2026-09-24T11:00:00Z');
  const card = {
    id: 'anthropic', title: 'Claude', plan: '', stale: false, error: '', rows: [{
      kind: 'metric', key: 'session', label: 'Session', headline: 'percent',
      usedPercent: 46, leftPercent: 54, resetAt: '2026-09-24T12:00:00Z',
      reset: '', detail: '', severity: 'low', value: '46%', window: 18_000,
    }],
  };
  const payload = {
    entries: [{ id: 'anthropic', shortName: 'cld', status: 'ready' }],
    primary: 'anthropic', generatedAt: nowMs, nextRefreshAt: nowMs + 60_000,
    hostError: '', version: 'test',
  };

  function note(resetTimes) {
    const html = renderToStaticMarkup(
      React.createElement(LanguageProvider, { language: 'pt-BR' },
        React.createElement(MacDashboard, {
          cards: [card], layout: { resetTimes, timeFormat: '24' }, nowMs, payload,
          onOpenCustomize() {}, onOpenSettings() {},
        })),
    );
    return html.match(/<div class="mac-metric-note"><span>([^<]+)<\/span>/)?.[1];
  }

  assert.equal(note('exact'), 'Redefine hoje às 12:00');
  assert.equal(note('countdown'), 'Redefine em 1h 0m');
  const withGoal = renderToStaticMarkup(React.createElement(LanguageProvider, { language: 'pt-BR' },
    React.createElement(MacDashboard, {
      cards: [card], layout: { ...emptyLayout(), usageGoal: true }, nowMs, payload,
      onOpenCustomize() {}, onOpenSettings() {},
    })));
  assert.match(withGoal, /Meta agora<\/span><strong>80%<\/strong>/);
  assert.match(withGoal, /class="mac-goal-meter" role="progressbar"/);
  assert.doesNotMatch(renderToStaticMarkup(React.createElement(LanguageProvider, { language: 'pt-BR' },
    React.createElement(MacDashboard, {
      cards: [card], layout: emptyLayout(), nowMs, payload,
      onOpenCustomize() {}, onOpenSettings() {},
    }))), /mac-goal-meter/);
  const settingsPayload = { ...emptyPayload(''), os: 'macos' };
  const settingsProps = {
    cards: [card], layout: emptyLayout(), nowMs, payload: settingsPayload,
    resetArmed: false,
    onAlwaysShowPace() {}, onUsageGoal() {}, onLanguage() {}, onOpenCustomize() {},
    onOpenProvider() {}, onReorderProviders() {}, onToggleProvider() {},
    onResetCustomization() {}, onResetTimes() {}, onShowAs() {},
    onTheme() {}, onTimeFormat() {}, onTabChange() {},
  };
  function settingsTab(tab) {
    return renderToStaticMarkup(React.createElement(TooltipProvider, {},
      React.createElement(LanguageProvider, { language: 'pt-BR' },
        React.createElement(Settings, { ...settingsProps, tab }))));
  }
  const general = settingsTab('general');
  assert.match(general, /role="tablist"/);
  assert.match(general, /Iniciar ao entrar/);
  assert.doesNotMatch(general, /Alertas de limite/);
  assert.doesNotMatch(general, /Redefinir toda a personalização/);
  const providers = settingsTab('providers');
  assert.match(providers, /Redefinir toda a personalização/);
  assert.match(providers, /Claude/);
  assert.doesNotMatch(providers, /Iniciar ao entrar/);
  const alerts = settingsTab('alerts');
  assert.match(alerts, /Alertas de limite/);
  assert.doesNotMatch(alerts, /Iniciar ao entrar/);
  const menu = settingsTab('menu');
  assert.match(menu, /Barra de menus/);
  assert.doesNotMatch(menu, /Exibição do uso/);
  const preferences = settingsTab('preferences');
  assert.match(preferences, /Aparência/);
  assert.match(preferences, /Exibição do uso/);
  assert.match(preferences, /Meta de uso/);
  assert.doesNotMatch(preferences, /Barra de menus/);
  console.log('macOS dashboard reset display: ok');
} finally {
  await server.close();
}
