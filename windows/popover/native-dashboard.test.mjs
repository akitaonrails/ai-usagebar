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
  const { LanguageProvider, translateUsage } = await server.ssrLoadModule('/src/lib/i18n.tsx');
  const { emptyLayout, emptyPayload } = await server.ssrLoadModule('/src/model.js');
  // Usage strings from the report reach the page in English and are translated for display.
  assert.equal(translateUsage('es', '54% left · 46% used · Limit reached'), '54% restante · 46% usado · Límite alcanzado');
  // The pt-BR "$X of $Y used" rewrite was dead: its leading `\b` could never
  // match before a `$` (#380). Assert it actually fires, at the start of a
  // string and mid-string, and that other locales are left alone.
  assert.equal(translateUsage('pt-BR', '$5.00 of $20.00 used'), '$5.00 de $20.00 usados');
  assert.equal(
    translateUsage('pt-BR', '46% used · $5.00 of $20.00 used'),
    '46% usados · $5.00 de $20.00 usados',
  );
  assert.equal(translateUsage('en', '$5.00 of $20.00 used'), '$5.00 of $20.00 used');
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

  function sectionMarkup(layout, language = 'pt-BR', section = card, account = undefined) {
    return renderToStaticMarkup(React.createElement(TooltipProvider, {},
      React.createElement(LanguageProvider, { language },
        React.createElement(ProviderSection, {
          card: section, layout, nowMs, account,
          onCustomize() {}, onRowAction() {}, onRowMenuOpenChange() {},
          onSwitchAccount() {}, onToggleCollapse() {}, onToggleShowAs() {},
        }))));
  }

  for (const popoverStyle of ['classic', 'native']) {
    const header = sectionMarkup({ ...emptyLayout(), popoverStyle }, 'en');
    // The header keeps only the Customize shortcut; the per-card Reset button is gone.
    assert.doesNotMatch(header, /aria-label="Reset Claude"/);
    assert.match(header, /aria-label="Customize Claude"/);
    // 4h of the 5h window have passed: the goal reads 20% left beside `54% restantes`,
    // and 80% used once the meter shows what is used.
    const withGoal = sectionMarkup({ ...emptyLayout(), popoverStyle, usageGoal: true });
    assert.match(withGoal, /Meta agora<\/span><strong class="font-semibold">20%<\/strong>/);
    assert.match(withGoal, /class="usage-goal-meter-fill" style="width:20%"/);
    const usedGoal = sectionMarkup({ ...emptyLayout(), popoverStyle, usageGoal: true, showAs: 'used' });
    assert.match(usedGoal, /Meta agora<\/span><strong class="font-semibold">80%<\/strong>/);
    assert.match(withGoal, /class="usage-goal-meter[^\"]*" role="progressbar"/);
    const withoutGoal = sectionMarkup({ ...emptyLayout(), popoverStyle, usageGoal: false });
    assert.doesNotMatch(withoutGoal, /usage-goal-meter|Meta agora/);
  }
  // Desktop switches use explicit text; row stars retain their independent
  // meter-selection meaning. Both usage windows survive all switch states.
  const desktopCard = { ...card, id: 'anthropic@desktop-2', title: 'Claude · desktop-2 (desktop)',
    rows: [card.rows[0], { ...card.rows[0], key: 'weekly', label: 'Weekly', window: 604800 }] };
  const desktopAccount = { vendor: 'anthropic-desktop', label: 'desktop-2', active: false, switching: false, busy: false, error: '' };
  for (const popoverStyle of ['classic', 'native']) {
    const layout = { ...emptyLayout(), popoverStyle };
    const inactive = sectionMarkup(layout, 'en', desktopCard, desktopAccount);
    assert.match(inactive, /aria-label="Switch Claude Desktop to desktop-2"/);
    assert.match(inactive, />Switch<\/button>/);
    assert.match(inactive, />Session</);
    assert.match(inactive, />Weekly</);
    const active = sectionMarkup(layout, 'en', desktopCard, { ...desktopAccount, active: true });
    assert.match(active, />Active<\/span>/);
    assert.doesNotMatch(active, /aria-label="Switch Claude Desktop/);
    const activeFailed = sectionMarkup(layout, 'en', desktopCard, {
      ...desktopAccount, active: true, error: 'Claude could not be reopened',
    });
    assert.match(activeFailed, /desktop-account-control is-current is-failed/);
    assert.match(activeFailed, /Claude could not be reopened\. Open Claude Desktop manually/);
    assert.match(activeFailed, />Active<\/span>/);
    const pending = sectionMarkup(layout, 'en', desktopCard, { ...desktopAccount, switching: true });
    assert.match(pending, /role="status"/);
    assert.match(pending, /Switching…/);
    const busy = sectionMarkup(layout, 'en', desktopCard, { ...desktopAccount, busy: true });
    assert.match(busy, /disabled="" aria-label="Switch Claude Desktop/);
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
        onRowAction() {}, onRowMenuOpenChange() {}, onSwitchAccount() {},
        onToggleCollapse() {}, onToggleShowAs() {},
      }))));
  // Provider tabs carry the logo and value; the full name is the accessible label, never the short code.
  assert.match(nativeDashboard, /role="group" aria-label="Provedores"/);
  // The default Left reading: 46% used reads 54%, like the meter below the tabs.
  assert.match(nativeDashboard, /aria-label="Claude 54%"/);
  assert.doesNotMatch(nativeDashboard, />cld</);
  assert.match(nativeDashboard, /data-card-id="anthropic"/);
  assert.match(nativeDashboard, /aria-expanded="true"/);
  // The tab reads the highest-percent window, like the Quattro bar: Z.AI with the weekly limit
  // spent and the 5h session idle read "0%". A grouped row never outranks a quota window.
  const quotaRow = (label, usedPercent, extra = {}) => ({
    ...card.rows[0], key: `metric:${label}`, label, usedPercent, leftPercent: 100 - usedPercent, ...extra,
  });
  const spentWeekly = {
    ...card, id: 'zai', title: 'Z.AI',
    rows: [quotaRow('Session', 0), quotaRow('Weekly', 100), quotaRow('Context', 100, { grouped: true }), quotaRow('MCP', 18)],
  };
  const groupedOnly = { ...card, id: 'supergrok', title: 'SuperGrok', rows: [quotaRow('Grok Build', 7, { grouped: true })] };
  const quotaDashboard = (showAs) => renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'pt-BR' },
      React.createElement(NativeDashboard, {
        cards: [spentWeekly, groupedOnly, { ...card, rows: [quotaRow('Session', 0), quotaRow('Context', 90, { grouped: true })] }],
        hint: false, layout: { ...emptyLayout(), popoverStyle: 'native', showAs }, nowMs, payload,
        onCustomizeProvider() {}, onDismissHint() {}, onOpenCustomize() {}, onOpenSettings() {},
        onRowAction() {}, onRowMenuOpenChange() {}, onSwitchAccount() {},
        onToggleCollapse() {}, onToggleShowAs() {},
      }))));
  const zaiDashboard = quotaDashboard('used');
  assert.match(zaiDashboard, /aria-label="Z.AI 100%"/);
  assert.match(zaiDashboard, /aria-label="SuperGrok 7%"/);
  assert.match(zaiDashboard, /aria-label="Claude 0%"/);
  // In the Left reading the tab shows what is left of that same most-used window: the spent
  // weekly limit reads 0%, never the idle session's 100%.
  const leftDashboard = quotaDashboard('left');
  assert.match(leftDashboard, /aria-label="Z.AI 0%"/);
  assert.match(leftDashboard, /aria-label="SuperGrok 93%"/);
  assert.match(leftDashboard, /aria-label="Claude 100%"/);
  // A value headline (a prepaid balance meter) is a figure, not a quota window: the tab keeps
  // ranking percent windows in either reading — the rule the menu-bar chip follows — and a
  // provider with only a balance chips its figure, the same either way.
  const valueRow = (label, usedPercent) => ({
    ...card.rows[0], key: `metric:${label}`, label, usedPercent, leftPercent: 100 - usedPercent,
    headline: 'value', value: '$0.25',
  });
  const valueDashboard = (showAs) => renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'pt-BR' },
      React.createElement(NativeDashboard, {
        cards: [
          { ...card, id: 'deepseek', title: 'DeepSeek', rows: [valueRow('Balance', 95), quotaRow('Weekly', 12)] },
          { ...card, id: 'openrouter', title: 'OpenRouter', rows: [valueRow('Credit balance', 40)] },
        ],
        hint: false, layout: { ...emptyLayout(), popoverStyle: 'native', showAs }, nowMs, payload,
        onCustomizeProvider() {}, onDismissHint() {}, onOpenCustomize() {}, onOpenSettings() {},
        onRowAction() {}, onRowMenuOpenChange() {}, onSwitchAccount() {},
        onToggleCollapse() {}, onToggleShowAs() {},
      }))));
  assert.match(valueDashboard('used'), /aria-label="DeepSeek 12%"/);
  assert.match(valueDashboard('left'), /aria-label="DeepSeek 88%"/);
  assert.match(valueDashboard('used'), /aria-label="OpenRouter \$0\.25"/);
  assert.match(valueDashboard('left'), /aria-label="OpenRouter \$0\.25"/);

  // A metric hidden in Customize never counts: Z.AI with Session and Weekly at 0% and the
  // monthly MCP window at 18% switched off reads 0% used (100% left), not 18%. A hidden window
  // with the highest percent loses to the visible ones; with every metric hidden the tab falls
  // back to what the card still shows (a balance, else a dash).
  const idleZai = {
    ...card, id: 'zai', title: 'Z.AI',
    rows: [quotaRow('Session', 0), quotaRow('Weekly', 0), quotaRow('MCP', 18)],
  };
  const busyKimi = {
    ...card, id: 'kimi', title: 'Kimi',
    rows: [quotaRow('Session', 30), quotaRow('Weekly', 30), quotaRow('MCP', 90)],
  };
  const allHidden = {
    ...card, id: 'openai', title: 'Codex',
    rows: [quotaRow('Session', 40), { kind: 'text', key: 'text:Balance', label: 'Balance', value: '$7' }],
  };
  const nothingLeft = { ...card, id: 'cursor', title: 'Cursor', rows: [quotaRow('Session', 40)] };
  const hiddenDashboard = (showAs, rows) => renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'pt-BR' },
      React.createElement(NativeDashboard, {
        cards: [idleZai, busyKimi, allHidden, nothingLeft],
        hint: false, layout: { ...emptyLayout(), popoverStyle: 'native', showAs, rows }, nowMs, payload,
        onCustomizeProvider() {}, onDismissHint() {}, onOpenCustomize() {}, onOpenSettings() {},
        onRowAction() {}, onRowMenuOpenChange() {}, onSwitchAccount() {},
        onToggleCollapse() {}, onToggleShowAs() {},
      }))));
  const off = (...keys) => ({ always: [], demand: [], off: Object.fromEntries(keys.map((key) => [key, true])) });
  const hiddenRows = {
    zai: off('metric:MCP'),
    kimi: off('metric:MCP'),
    openai: off('metric:Session'),
    cursor: off('metric:Session'),
  };
  const hiddenUsed = hiddenDashboard('used', hiddenRows);
  assert.match(hiddenUsed, /aria-label="Z.AI 0%"/);
  assert.match(hiddenUsed, /aria-label="Kimi 30%"/);
  assert.match(hiddenUsed, /aria-label="Codex \$7"/);
  assert.match(hiddenUsed, /aria-label="Cursor —"/);
  assert.match(hiddenDashboard('left', hiddenRows), /aria-label="Z.AI 100%"/);
  // Nothing hidden: the tab still reads the busiest window.
  const unhidden = hiddenDashboard('used', {});
  assert.match(unhidden, /aria-label="Z.AI 18%"/);
  assert.match(unhidden, /aria-label="Kimi 90%"/);
  assert.match(unhidden, /aria-label="Codex 40%"/);
  // A waiting release shows the same Update available card as Classic, above the provider tabs.
  const withUpdate = renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'en' },
      React.createElement(NativeDashboard, {
        cards: [card], hint: false, layout: { ...emptyLayout(), popoverStyle: 'native' }, nowMs,
        payload: { ...payload, repository: 'akitaonrails/ai-usagebar', update: { version: '9.9.9', state: 'available', url: '', installable: true, error: '' } },
        onCustomizeProvider() {}, onDismissHint() {}, onOpenCustomize() {}, onOpenSettings() {},
        onRowAction() {}, onRowMenuOpenChange() {}, onSwitchAccount() {},
        onToggleCollapse() {}, onToggleShowAs() {},
      }))));
  assert.match(withUpdate, />Update available</);
  assert.ok(withUpdate.indexOf('>Update available<') < withUpdate.indexOf('native-provider-tabs'));
  assert.doesNotMatch(nativeDashboard, /Atualização disponível/);
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
  // An empty payload reads as the host default, the chart.
  assert.match(menu, /Gráfico/);
  assert.doesNotMatch(menu, /Período de uso|Provedor em foco/);
  assert.doesNotMatch(menu, /Identificar provedores por|Mostrar todos os provedores|Ocultar valor de uso/);
  const menuEnglish = settingsTab('menu', settingsProps, 'en');
  assert.match(menuEnglish, /Menu Bar Shows/);
  assert.match(menuEnglish, /Chart/);
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
    payload: { ...settingsPayload, menuBarLook: 'chart' },
  });
  assert.match(chartMenuBar, /Barra de menus mostra/);
  assert.match(chartMenuBar, /Gráfico/);
  assert.doesNotMatch(chartMenuBar, /Identificar provedores por|Mostrar todos os provedores|Ocultar valor de uso/);
  const providersMenuBar = settingsTab('general', {
    ...settingsProps,
    layout: { ...emptyLayout(), popoverStyle: 'classic' },
    payload: { ...settingsPayload, menuBarLook: 'logos' },
  });
  assert.match(providersMenuBar, /Logotipos/);
  assert.doesNotMatch(providersMenuBar, /Identificar provedores por|Mostrar todos os provedores|Ocultar valor de uso/);
  // The Quattro look is a third choice of the same picker, not a separate set of controls.
  const quattroMenuBar = settingsTab('general', {
    ...settingsProps,
    layout: { ...emptyLayout(), popoverStyle: 'classic' },
    payload: { ...settingsPayload, menuBarLook: 'quattro' },
  });
  assert.match(quattroMenuBar, /Barra de menus mostra/);
  assert.match(quattroMenuBar, /Quattro/);
  assert.doesNotMatch(quattroMenuBar, /Identificar provedores por|Mostrar todos os provedores|Ocultar valor de uso/);
  // The short-name switch belongs to the Quattro look only.
  assert.match(quattroMenuBar, /Mostrar nome curto/);
  assert.doesNotMatch(chartMenuBar, /Mostrar nome curto/);
  assert.doesNotMatch(providersMenuBar, /Mostrar nome curto/);
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
