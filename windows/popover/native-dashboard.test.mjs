import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

process.env.TZ = 'UTC';
const server = await createServer({ server: { middlewareMode: true, hmr: false, ws: false }, appType: 'custom' });

try {
  const { NativeDashboard } = await server.ssrLoadModule('/src/screens/NativeDashboard.tsx');
  const { ProviderSection } = await server.ssrLoadModule('/src/components/ProviderSection.tsx');
  const { Settings } = await server.ssrLoadModule('/src/screens/Settings.tsx');
  const { Footer } = await server.ssrLoadModule('/src/components/Chrome.tsx');
  const { TooltipProvider } = await server.ssrLoadModule('/src/components/ui/tooltip.tsx');
  const { LanguageProvider } = await server.ssrLoadModule('/src/lib/i18n.tsx');
  const { emptyLayout, emptyPayload } = await server.ssrLoadModule('/src/model.js');
  const nowMs = Date.parse('2026-09-24T11:00:00Z');
  const card = {
    id: 'anthropic', title: 'Claude', plan: '', stale: false, error: '', rows: [{
      kind: 'metric', key: 'session', label: 'Session', headline: 'percent',
      usedPercent: 46, leftPercent: 54, resetAt: '2026-09-24T12:00:00Z',
      reset: '', detail: '', severity: 'low', value: '46%', window: 18_000,
    }, { kind: 'text', key: 'balance', label: 'Balance', value: '$12.50' }],
  };
  const payload = {
    entries: [{ id: 'anthropic', shortName: 'cld', status: 'ready' }],
    primary: 'anthropic', generatedAt: nowMs, nextRefreshAt: nowMs + 60_000,
    hostError: '', version: 'test',
  };

  function sectionMarkup(layout, language = 'pt-BR', section = card) {
    return renderToStaticMarkup(React.createElement(TooltipProvider, {},
      React.createElement(LanguageProvider, { language },
        React.createElement(ProviderSection, {
          card: section, layout, nowMs,
          onCustomize() {}, onReset() {}, onRowAction() {}, onRowMenuOpenChange() {},
          onSwitchAccount() {}, onToggleCollapse() {}, onToggleShowAs() {},
        }))));
  }

  for (const popoverStyle of ['classic', 'native']) {
    const withGoal = sectionMarkup({ ...emptyLayout(), popoverStyle, usageGoal: true });
    assert.match(withGoal, /Meta agora<\/span><strong class="font-semibold">80%<\/strong>/);
    assert.match(withGoal, /class="usage-goal-meter[^\"]*" role="progressbar"/);
    const withoutGoal = sectionMarkup({ ...emptyLayout(), popoverStyle, usageGoal: false });
    assert.doesNotMatch(withoutGoal, /usage-goal-meter|Meta agora/);
  }
  assert.match(sectionMarkup({ ...emptyLayout(), resetTimes: 'exact', timeFormat: '24' }), /Redefine hoje às 12:00/);
  assert.match(sectionMarkup({ ...emptyLayout(), resetTimes: 'countdown', timeFormat: '24' }), /Redefine em 1h 0m/);
  // The reset text carries its own hint (the exact time): an outer Hint around TruncatedText
  // reached nothing, because the component forwards no trigger props.
  assert.match(sectionMarkup({ ...emptyLayout(), resetTimes: 'countdown', timeFormat: '24' }), /data-slot="tooltip-trigger"[^>]*>Redefine em 1h 0m</);
  const withCredits = { ...card, rows: [...card.rows, { kind: 'resetCredits', label: 'Rate Limit Resets', available: 1, credits: [] }] };
  assert.match(sectionMarkup(emptyLayout(), 'pt-BR', withCredits), />Redefinições de limite</);
  assert.match(sectionMarkup(emptyLayout(), 'pt-BR', withCredits), />1 disponível</);
  assert.match(sectionMarkup(emptyLayout(), 'en', withCredits), />Rate Limit Resets</);
  assert.match(sectionMarkup(emptyLayout(), 'en', withCredits), />1 available</);

  const nativeDashboard = renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'pt-BR' },
      React.createElement(NativeDashboard, {
        cards: [card], hint: false, layout: { ...emptyLayout(), popoverStyle: 'native' }, nowMs, payload,
        onCustomizeProvider() {}, onDismissHint() {}, onOpenCustomize() {}, onOpenSettings() {},
        onResetProvider() {}, onRowAction() {}, onRowMenuOpenChange() {}, onSwitchAccount() {},
        onToggleCollapse() {}, onToggleShowAs() {},
      }))));
  // Provider tabs carry the logo and value; the full name is the accessible label, never the short code.
  assert.match(nativeDashboard, /role="group" aria-label="Provedores"/);
  assert.match(nativeDashboard, /aria-label="Claude 46%"/);
  assert.doesNotMatch(nativeDashboard, />cld</);
  assert.match(nativeDashboard, /data-card-id="anthropic"/);
  assert.match(nativeDashboard, /aria-expanded="true"/);
  const settingsPayload = { ...emptyPayload(''), os: 'macos' };
  const settingsProps = {
    cards: [card], layout: { ...emptyLayout(), popoverStyle: 'native' }, nowMs, payload: settingsPayload,
    resetArmed: false,
    onAlwaysShowPace() {}, onUsageGoal() {}, onLanguage() {}, onOpenCustomize() {},
    onOpenProvider() {}, onReorderProviders() {}, onToggleProvider() {},
    onResetCustomization() {}, onResetTimes() {}, onShowAs() {},
    onTheme() {}, onTimeFormat() {}, onPopoverStyle() {}, onTabChange() {},
  };
  function settingsTab(tab, settings = settingsProps, language = 'pt-BR') {
    return renderToStaticMarkup(React.createElement(TooltipProvider, {},
      React.createElement(LanguageProvider, { language },
        React.createElement(Settings, { ...settings, tab }))));
  }
  const general = settingsTab('general');
  assert.match(general, /role="tablist"/);
  assert.equal((general.match(/role="tab"/g) || []).length, 5);
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
  assert.match(menu, /Barra de menus mostra/);
  assert.match(menu, /Logotipos/);
  assert.doesNotMatch(menu, /Período de uso|Provedor em foco/);
  assert.doesNotMatch(menu, /Identificar provedores por|Mostrar todos os provedores|Ocultar valor de uso/);
  const menuEnglish = settingsTab('menu', settingsProps, 'en');
  assert.match(menuEnglish, /Menu Bar Shows/);
  assert.match(menuEnglish, /Logos/);
  assert.doesNotMatch(menuEnglish, /Usage Window|Focused Provider|Highest consumption/);
  const preferences = settingsTab('preferences');
  assert.match(preferences, /Aparência/);
  assert.match(preferences, /Exibição do uso/);
  assert.match(preferences, /Meta de uso/);
  assert.doesNotMatch(preferences, /Barra de menus/);
  const windowsNative = settingsTab('general', {
    ...settingsProps,
    payload: { ...emptyPayload(''), os: 'windows' },
  });
  assert.equal((windowsNative.match(/role="tab"/g) || []).length, 3);
  const macClassic = settingsTab('general', {
    ...settingsProps,
    layout: { ...emptyLayout(), popoverStyle: 'classic' },
  });
  assert.doesNotMatch(macClassic, /role="tab"/);
  assert.match(macClassic, /Barra de menus/);
  assert.match(macClassic, /Meta de uso/);
  // Mutation captured: reversing the chart-mode gate exposes names-only menu controls in chart mode.
  const chartMenuBar = settingsTab('general', {
    ...settingsProps,
    layout: { ...emptyLayout(), popoverStyle: 'classic' },
    payload: { ...settingsPayload, menuBarChart: true },
  });
  assert.match(chartMenuBar, /Barra de menus mostra/);
  assert.match(chartMenuBar, /Gráfico/);
  assert.doesNotMatch(chartMenuBar, /Identificar provedores por|Mostrar todos os provedores|Ocultar valor de uso/);
  const providersMenuBar = settingsTab('general', {
    ...settingsProps,
    layout: { ...emptyLayout(), popoverStyle: 'classic' },
    payload: { ...settingsPayload, menuBarChart: false },
  });
  assert.match(providersMenuBar, /Logotipos/);
  assert.doesNotMatch(providersMenuBar, /Identificar provedores por|Mostrar todos os provedores|Ocultar valor de uso/);
  const footerMarkup = renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'en' },
      React.createElement(Footer, {
        nowMs, optionsOpen: true, payload: settingsPayload, popoverStyle: 'classic', updatePending: false,
        onOpenAbout() {}, onCheckUpdates() {}, onOpenCustomize() {}, onOpenSettings() {},
        onOptionsOpenChange() {},
      }))));
  if (footerMarkup.includes('role="menu"')) {
    assert.doesNotMatch(footerMarkup, /Native Style/);
  } else {
    // Radix keeps its portaled menu out of static markup; inspect the Footer's menu source.
    const chromeSource = await readFile(new URL('./src/components/Chrome.tsx', import.meta.url), 'utf8');
    assert.doesNotMatch(chromeSource, /m\.native_style\(\)|Native Style/);
  }
  console.log('native dashboard: ok');
} finally {
  await server.close();
}
