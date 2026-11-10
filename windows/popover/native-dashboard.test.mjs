import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

process.env.TZ = 'UTC';
const server = await createServer({ server: { middlewareMode: true, hmr: false, ws: false }, appType: 'custom' });

try {
  const { NativeDashboard } = await server.ssrLoadModule('/src/screens/NativeDashboard.tsx');
  const { Dashboard } = await server.ssrLoadModule('/src/screens/Dashboard.tsx');
  const { ProviderSection } = await server.ssrLoadModule('/src/components/ProviderSection.tsx');
  const { Settings } = await server.ssrLoadModule('/src/screens/Settings.tsx');
  const { About } = await server.ssrLoadModule('/src/screens/About.tsx');
  const { Footer, TopBar } = await server.ssrLoadModule('/src/components/Chrome.tsx');
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

  function sectionMarkup(layout, language = 'pt-BR', section = card, account = null) {
    return renderToStaticMarkup(React.createElement(TooltipProvider, {},
      React.createElement(LanguageProvider, { language },
        React.createElement(ProviderSection, {
          account, card: section, layout, nowMs,
          onCustomize() {}, onRowAction() {}, onRowMenuOpenChange() {},
          onSwitchAccount() {}, onToggleCollapse() {}, onToggleShowAs() {},
        }))));
  }

  // The one icon button of the popover (IconButton.tsx): the same <button> tag
  // carries the accessible name, the icon-btn box and the tooltip trigger — the
  // assertions read each opening tag whole, so attribute order is no contract.
  const buttonTags = (markup) => markup.match(/<button[^>]*>/g) ?? [];
  const iconButtonTag = (markup, label) => {
    const tags = buttonTags(markup).filter((tag) => tag.includes(`aria-label="${label}"`));
    assert.ok(tags.length > 0, `expected an icon button named "${label}"`);
    for (const tag of tags) {
      assert.match(tag, /class="[^"]*icon-btn[^"]*"/, `the "${label}" button is the icon-btn box`);
      assert.match(tag, /data-slot="tooltip-trigger"/, `the "${label}" button triggers the tooltip`);
    }
    return tags;
  };

  for (const popoverStyle of ['classic', 'native']) {
    const header = sectionMarkup({ ...emptyLayout(), popoverStyle }, 'en');
    // The header keeps only the Customize shortcut; the per-card Reset button is gone.
    assert.doesNotMatch(header, /aria-label="Reset Claude"/);
    assert.match(header, /aria-label="Customize Claude"/);
    // The header's shortcut is the uniform icon button and keeps the
    // header-action hook (the image export and the actions rely on it).
    const headerActionTag = buttonTags(header).find((tag) => tag.includes('aria-label="Customize Claude"'));
    assert.ok(headerActionTag, 'the header renders the Customize action');
    assert.match(headerActionTag, /class="[^"]*icon-btn[^"]*"/);
    assert.match(headerActionTag, /class="[^"]*header-action[^"]*"/);
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
  // The copy-as-image action sits in the header at rest, not only on hover.
  const withCopy = renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'en' },
      React.createElement(ProviderSection, {
        account: null, card, layout: emptyLayout(), nowMs,
        onCopyImage() {}, onCustomize() {}, onRowAction() {}, onRowMenuOpenChange() {},
        onSwitchAccount() {}, onToggleCollapse() {}, onToggleShowAs() {},
      }))));
  const copyButton = withCopy.match(/<button[^>]*aria-label="Copy as image"[^>]*>/);
  assert.ok(copyButton, 'the header renders the copy action');
  assert.doesNotMatch(copyButton[0], /opacity-0/);
  // An inactive Claude Desktop account with a trusted CLI configured gets a
  // Prepare button beside the switch star; both usage windows survive every
  // prepare state.
  const desktopCard = { ...card, id: 'anthropic@desktop-2', title: 'Claude · desktop-2 (desktop)',
    rows: [card.rows[0], { ...card.rows[0], key: 'weekly', label: 'Weekly', window: 604800 }] };
  const desktopAccount = { vendor: 'claude-desktop', label: 'desktop-2', active: false, switching: false, busy: false, error: '' };
  for (const popoverStyle of ['classic', 'native']) {
    const layout = { ...emptyLayout(), popoverStyle };
    const unusedCard = { ...desktopCard, rows: desktopCard.rows.map((row) => ({ ...row, resetAt: '', usedPercent: 0 })) };
    const prepareAccount = { ...desktopAccount, prepareEnabled: true };
    const prepare = sectionMarkup(layout, 'en', unusedCard, prepareAccount);
    assert.match(prepare, /aria-label="Prepare Claude Desktop account desktop-2"/);
    assert.match(prepare, /Prepare<\/button>/);
    assert.match(prepare, />Session</);
    assert.match(prepare, />Weekly</);
    assert.doesNotMatch(sectionMarkup(layout, 'en', unusedCard, { ...prepareAccount, active: true }), /Prepare Claude Desktop/);
    const preparing = sectionMarkup(layout, 'en', unusedCard, { ...prepareAccount, preparing: true, busy: true });
    assert.match(preparing, /disabled="" aria-label="Preparing… Claude Desktop/);
    const ready = sectionMarkup(layout, 'en', desktopCard, prepareAccount);
    assert.match(ready, /disabled="" aria-label="Ready Claude Desktop/);
    const retry = sectionMarkup(layout, 'en', unusedCard, { ...prepareAccount, prepareError: 'Proxy unavailable' });
    assert.match(retry, /desktop-account-control is-failed/);
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
  // The header's Refresh and Settings are the uniform icon buttons, with tooltips.
  iconButtonTag(nativeDashboard, 'Atualizar');
  iconButtonTag(nativeDashboard, 'Configurações');
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
  // #453: an exhausted per-model pool (Gemini 4 Argon) is one model's limit, not the provider's:
  // the tab reads the busiest shared window, and the pool stands in only when it is all there is.
  // Antigravity's rows sit under its Session / Weekly headings, so all of them are grouped.
  const antigravityTab = (rows) => renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'pt-BR' },
      React.createElement(NativeDashboard, {
        cards: [{ ...card, id: 'antigravity', title: 'Antigravity', rows }],
        hint: false, layout: { ...emptyLayout(), popoverStyle: 'native', showAs: 'used' }, nowMs, payload,
        onCustomizeProvider() {}, onDismissHint() {}, onOpenCustomize() {}, onOpenSettings() {},
        onRowAction() {}, onRowMenuOpenChange() {}, onSwitchAccount() {},
        onToggleCollapse() {}, onToggleShowAs() {},
      }))));
  const argon = quotaRow('Gemini 4 Argon', 100, { grouped: true, modelPool: true });
  assert.match(antigravityTab([
    quotaRow('Gemini', 55, { grouped: true }), quotaRow('Gemini', 39, { grouped: true }), argon,
  ]), /aria-label="Antigravity 55%"/);
  assert.match(antigravityTab([argon]), /aria-label="Antigravity 100%"/);
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

  // A provider with several accounts is one tab (named after the provider) and its section
  // gets the account tabs under the header — no dot, no bullet: the label is the tab. The
  // card shows the selected account's metrics; with one account there are no tabs at all.
  const workCard = {
    ...card, id: 'anthropic@work', title: 'Claude · work',
    rows: [quotaRow('Session', 90), { ...card.rows[1] }],
  };
  const multiAccount = (accountTab = {}, primary = payload.primary) => renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'en' },
      React.createElement(NativeDashboard, {
        cards: [card, workCard, { ...card, id: 'zai', title: 'Z.AI', rows: [quotaRow('Session', 10)] }],
        hint: false,
        layout: { ...emptyLayout(), popoverStyle: 'native', accountTab },
        nowMs, payload: { ...payload, primary },
        onCustomizeProvider() {}, onDismissHint() {}, onOpenCustomize() {}, onOpenSettings() {},
        onRowAction() {}, onRowMenuOpenChange() {}, onSelectAccount() {},
        onSwitchAccount() {}, onToggleCollapse() {}, onToggleShowAs() {},
      }))));
  const tabs = multiAccount();
  // One provider tab per provider, not one per account: Claude and Z.AI.
  assert.equal((tabs.match(/class="native-tab"/g) || []).length, 2);
  // ASSERT: the section header names the provider, never the account's "Claude · work" title.
  assert.doesNotMatch(tabs, /Claude · work/);
  // ASSERT: the account tabs live under the header, labelled Default (no suffix) and work.
  assert.match(tabs, /role="tablist" aria-label="Accounts"/);
  assert.match(tabs, />Default</);
  assert.match(tabs, />work</);
  // ASSERT: the tablist is plain — no dot or bullet standing in for a label.
  const tablist = tabs.slice(tabs.indexOf('role="tablist"'), tabs.indexOf('</div>', tabs.indexOf('role="tablist"')));
  assert.doesNotMatch(tablist, /aria-hidden|●|•/);
  // ASSERT: with no remembered tab the payload's primary is worth when it is one of the group's cards.
  assert.match(tabs, /data-card-id="anthropic"/);
  assert.doesNotMatch(tabs, /data-card-id="anthropic@work"/);
  // ASSERT: a remembered tab picks that account's card, and wins over the primary.
  assert.match(multiAccount({ anthropic: 'anthropic@work' }), /data-card-id="anthropic@work"/);
  assert.match(multiAccount({ anthropic: 'anthropic' }, 'anthropic@work'), /data-card-id="anthropic"/);
  // ASSERT: with nothing remembered the primary's account is the one shown.
  assert.match(multiAccount({}, 'anthropic@work'), /data-card-id="anthropic@work"/);
  // ASSERT: a remembered tab no account holds falls back to the primary, else the first card.
  assert.doesNotMatch(multiAccount({ anthropic: 'anthropic@gone' }), /data-card-id="anthropic@work"/);
  assert.match(multiAccount({ anthropic: 'anthropic@gone' }, 'anthropic@work'), /data-card-id="anthropic@work"/);
  // ASSERT: the single-account dashboard keeps no account tabs (the old screen).
  assert.doesNotMatch(nativeDashboard, /role="tablist"/);

  // One live account still titles the provider after the catalog. Account
  // labels belong to the account tabs, not the header or its tooltips.
  const liveAccounts = ['one', 'two'].map((id, index) => ({
    ...card, id: `antigravity@${id}`, icon: 'antigravity',
    title: `Antigravity · ${index ? 'b' : 'a'}***@example.com`,
  }));
  for (const [Component, popoverStyle] of [[Dashboard, 'classic'], [NativeDashboard, 'native']]) {
    for (const count of [1, 2]) {
      const cards = liveAccounts.slice(0, count);
      const markup = renderToStaticMarkup(React.createElement(TooltipProvider, {},
        React.createElement(LanguageProvider, { language: 'en' },
          React.createElement(Component, {
            cards, visible: cards, hint: false, layout: { ...emptyLayout(), popoverStyle }, nowMs,
            payload: { ...payload, primary: cards[0].id, providers: [{ id: 'antigravity', name: 'Antigravity' }] },
            onCustomizeProvider() {}, onDismissHint() {}, onOpenCustomize() {}, onOpenSettings() {},
            onRowAction() {}, onRowMenuOpenChange() {}, onSelectAccount() {}, onReorder() {},
            onSwitchAccount() {}, onToggleCollapse() {}, onToggleShowAs() {},
          }))));
      const header = markup.match(/<section\b[^>]*data-card-id="antigravity@one"[\s\S]*?<\/header>/)?.[0];
      assert.ok(header);
      assert.match(header, />Antigravity</);
      assert.doesNotMatch(header, /example\.com|Antigravity ·/);
      assert.match(markup, /data-card-id="antigravity@one"/);
      if (count === 2) {
        assert.match(markup, /role="tablist" aria-label="Accounts"/);
        assert.match(markup, /a\*\*\*@example\.com/);
        assert.match(markup, /b\*\*\*@example\.com/);
      } else {
        assert.doesNotMatch(markup, /role="tablist" aria-label="Accounts"/);
      }
    }
  }
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
  // The tab keeps its "Provedores" label and the list's own title is gone, so
  // the visible word appears exactly once (the tab), not twice.
  assert.equal(providers.split('>Provedores<').length - 1, 1);
  // The Providers header actions are the uniform icon buttons: reset (only
  // with cards) and Detect on its right, both with tooltips.
  iconButtonTag(providers, 'Redefinir toda a personalização');
  iconButtonTag(providers, 'Detectar provedores');
  assert.ok(providers.indexOf('Redefinir toda a personalização') < providers.indexOf('Detectar provedores'));
  // Armed: the confirm label on the data-armed box, not the reset label.
  const providersArmed = settingsTab('providers', { ...settingsProps, resetArmed: true });
  assert.match(iconButtonTag(providersArmed, 'Clique novamente para confirmar')[0], /data-armed=""/);
  assert.doesNotMatch(providersArmed, /aria-label="Redefinir toda a personalização"/);
  // Without cards there is nothing to reset: Detect only.
  const providersNoCards = settingsTab('providers', { ...settingsProps, cards: [] });
  assert.doesNotMatch(providersNoCards, /Redefinir toda a personalização|Clique novamente para confirmar/);
  iconButtonTag(providersNoCards, 'Detectar provedores');
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
  const iconMenuBar = settingsTab('menu', {
    ...settingsProps,
    payload: { ...settingsPayload, menuBarLook: 'icon' },
  });
  assert.match(iconMenuBar, /Ícone do app/);
  assert.doesNotMatch(iconMenuBar, /Mostrar nome curto/);
  // Contract: the native tray glyph must match the actual About SVG, even
  // after an icon-library update. Swapping either glyph breaks this check.
  const aboutMarkup = renderToStaticMarkup(React.createElement(LanguageProvider, { language: 'en' },
    React.createElement(About, { payload: { ...settingsPayload, repository: '' } })));
  const aboutMark = aboutMarkup.match(/<span class="[^"]*--app-mark[^"]*">([\s\S]*?)<\/span>/)[1];
  const nativeMark = await readFile(new URL('../../macos/menu-bar-icon.svg', import.meta.url), 'utf8');
  const svgPaths = (markup) => [...markup.matchAll(/<path\b[^>]*\bd="([^"]+)"/g)].map(match => match[1]);
  assert.deepEqual(svgPaths(nativeMark), svgPaths(aboutMark));
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
  // The TopBar's Back/Reset/Detect are the uniform icon buttons; Detect is
  // offered only by the screen (Customize Classic) and the armed reset wears
  // the confirm label on the data-armed box.
  const topBar = (props) => renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'en' },
      React.createElement(TopBar, { onBack() {}, title: 'Customize', ...props }))));
  const topBarFull = topBar({ onReset() {}, resetArmed: false, resetLabel: 'Reset All Customization', onDetect() {} });
  iconButtonTag(topBarFull, 'Back');
  iconButtonTag(topBarFull, 'Reset All Customization');
  iconButtonTag(topBarFull, 'Detect Providers');
  assert.ok(topBarFull.indexOf('Reset All Customization') < topBarFull.indexOf('Detect Providers'));
  // The fallback: without onDetect the bar keeps Back and Reset only.
  const topBarBare = topBar({ onReset() {}, resetLabel: 'Reset All Customization' });
  assert.doesNotMatch(topBarBare, /aria-label="Detect Providers"/);
  iconButtonTag(topBarBare, 'Back');
  iconButtonTag(topBarBare, 'Reset All Customization');
  assert.match(iconButtonTag(topBar({ onReset() {}, resetArmed: true, resetLabel: 'Reset All Customization', onDetect() {} }), 'Click again to confirm')[0], /data-armed=""/);
  // The wiring (static): App offers Detect only on the Classic Customize screen.
  const appSource = await readFile(new URL('./src/App.tsx', import.meta.url), 'utf8');
  assert.match(appSource, /onDetect=\{screen === "customize" && !native \?/);
  // The single provider list: an enabled row that still needs its credential
  // keeps the hint off the row's text — it rides an alert icon beside the name
  // — and its sign-in action takes the "+" column as an icon.
  const { ProviderList } = await server.ssrLoadModule('/src/components/ProviderList.tsx');
  const needsProviders = [
    { id: 'anthropic', name: 'Anthropic API', enabled: true, configured: false, env: '', signIn: 'Add an API key in Settings, then Refresh.', action: 'settings', addAccounts: ['code'] },
    { id: 'openai', name: 'Codex', enabled: true, configured: false, env: '', signIn: 'Run `codex login` in a terminal, then Refresh.', action: 'terminal', addAccounts: ['codex'] },
    { id: 'cursor', name: 'Cursor', enabled: true, configured: false, env: '', signIn: 'Sign in to the Cursor app, then Refresh.', action: 'app', addAccounts: [] },
    { id: 'zai', name: 'Z.AI', enabled: true, configured: false, env: 'ZAI_API_KEY', signIn: 'Set the ZAI_API_KEY, then Refresh.', action: '', addAccounts: [] },
    { id: 'moonshot', name: 'Moonshot', enabled: true, configured: true, env: '', signIn: '', action: '', addAccounts: ['code'] },
    { id: 'antigravity', name: 'Antigravity', enabled: true, configured: true, env: '', signIn: '', action: '', addAccounts: ['code', 'desktop'] },
    { id: 'deepseek', name: 'DeepSeek', enabled: true, configured: true, env: '', signIn: '', action: '', addAccounts: [] },
  ];
  // Every row already has its card, as once the report brings the error card.
  const needsCards = [
    { id: 'anthropic', title: 'Anthropic API', icon: 'anthropic_api', rows: [{ kind: 'metric' }] },
    { id: 'openai', title: 'Codex', icon: 'openai', rows: [{ kind: 'metric' }] },
    { id: 'cursor', title: 'Cursor', icon: 'cursor', rows: [{ kind: 'metric' }] },
    { id: 'zai', title: 'Z.AI', icon: 'zai', rows: [{ kind: 'metric' }] },
    { id: 'moonshot', title: 'Moonshot', icon: 'moonshot', rows: [{ kind: 'metric' }, { kind: 'metric' }] },
    { id: 'antigravity', title: 'Antigravity', icon: 'antigravity', rows: [{ kind: 'metric' }] },
  ];
  const listMarkup = renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'en' },
      React.createElement(ProviderList, {
        cards: needsCards, error: '', layout: emptyLayout(), providers: needsProviders,
        onOpen() {}, onReorder() {}, onToggle() {},
      }))));
  const rowOf = (id, nextId) => listMarkup.slice(
    listMarkup.indexOf(`data-provider-id="${id}"`),
    nextId ? listMarkup.indexOf(`data-provider-id="${nextId}"`) : listMarkup.length,
  );
  const escaped = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const keyHint = 'Add an API key in Settings, then Refresh.';
  // The hint is never visible text: once, as the alert icon's label (Radix
  // keeps the tooltip itself out of static markup).
  assert.equal(listMarkup.split(keyHint).length - 1, 1);
  assert.doesNotMatch(listMarkup, new RegExp(`>${escaped(keyHint)}`));
  const settingsRow = rowOf('anthropic', 'openai');
  assert.match(settingsRow, new RegExp(`<span tabindex="0" role="img" aria-label="${escaped(keyHint)}"`));
  assert.match(settingsRow, /<svg[^>]*class="[^"]*text-notice[^"]*"/);
  assert.match(settingsRow, /<button[^>]*aria-label="Add key"/);
  assert.doesNotMatch(settingsRow, /action-btn|aria-label="Add account"/);
  // The focusable alert icon is not nested in the row's open button: the
  // identity of a row that needs its credential is not a button.
  assert.doesNotMatch(settingsRow, /<button[^>]*>(?:(?!<\/button>).)*role="img"/s);
  const terminalRow = rowOf('openai', 'cursor');
  assert.match(terminalRow, /<button[^>]*aria-label="Sign in"/);
  assert.doesNotMatch(terminalRow, /action-btn|aria-label="Add account"/);
  assert.match(rowOf('cursor', 'zai'), /<button[^>]*aria-label="Open app"/);
  // Without an action the alert stays and the "+" column is an empty slot.
  const noActionRow = rowOf('zai', 'moonshot');
  assert.match(noActionRow, /<span tabindex="0" role="img" aria-label="Set the ZAI_API_KEY, then Refresh\."/);
  assert.doesNotMatch(noActionRow, /aria-label="(Add key|Sign in|Open app)"/);
  assert.match(noActionRow, /<span aria-hidden="true" class="size-\[var\(--icon-btn\)\]">/);
  // A ready row keeps its metrics subtitle and the add-account "+".
  const readyRow = rowOf('moonshot', 'antigravity');
  assert.match(readyRow, />2 metrics</);
  assert.match(readyRow, /<button[^>]*aria-label="Add account"/);
  assert.doesNotMatch(readyRow, /text-notice/);
  // The "+" column is the uniform icon button now: the direct "+" (one account
  // kind) and the menu trigger (two kinds) keep the name and the tooltip on
  // the button itself; the active chevron opens the provider's card the same
  // way, and a provider without a card keeps the column with the same box,
  // inert and out of the interaction.
  const directAdd = iconButtonTag(readyRow, 'Add account');
  assert.equal(directAdd.length, 1);
  assert.doesNotMatch(directAdd[0], /aria-haspopup/);
  const menuRow = rowOf('antigravity', 'deepseek');
  // The menu trigger is the same contract as the direct "+": the icon-btn box,
  // the name and the tooltip trigger on the button itself — the asChild merge
  // may not drop the tooltip's slot — plus the menu wiring on the same tag.
  const menuAdd = iconButtonTag(menuRow, 'Add account');
  assert.equal(menuAdd.length, 1);
  assert.match(menuAdd[0], /aria-haspopup="menu"/);
  iconButtonTag(readyRow, 'Open Moonshot');
  const inertRow = rowOf('deepseek');
  const inertTag = (inertRow.match(/<span[^>]*data-inert[^>]*>/g) ?? [])[0];
  assert.ok(inertTag, 'the inert chevron keeps its column');
  assert.match(inertTag, /aria-hidden="true"/);
  assert.match(inertTag, /class="[^"]*icon-btn[^"]*"/);
  assert.doesNotMatch(inertTag, /data-slot/);
  // The hint is no longer a text line: it rides the list's help icon in the
  // header — focusable, labelled with the hint itself, tooltip trigger — and
  // the group no longer carries a visible "Providers" title of its own.
  // "Detect Providers" is out of the list (the screen passes it as a header
  // action). An error stays the red text under the list; without one there is
  // no hint line at all.
  const providersHint = 'Turn a provider on to fetch it. Changes are saved to config.toml.';
  assert.equal(listMarkup.split(providersHint).length - 1, 1);
  const helpTag = (listMarkup.match(/<span[^>]*>/g) ?? []).find((tag) => tag.includes(`aria-label="${providersHint}"`));
  assert.ok(helpTag, 'the help icon carries the hint as its accessible name');
  assert.match(helpTag, /tabindex="0"/);
  assert.match(helpTag, /role="img"/);
  assert.match(helpTag, /data-slot="tooltip-trigger"/);
  assert.doesNotMatch(listMarkup, new RegExp(`>${escaped(providersHint)}`));
  // The group's visible "Providers" title is gone: the header keeps only the
  // help icon (plus the screen's header actions, when it passes them).
  assert.doesNotMatch(listMarkup, />Providers</);
  assert.doesNotMatch(listMarkup, /Detect Providers/);
  const listWithError = renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'en' },
      React.createElement(ProviderList, {
        cards: needsCards, error: 'Could not load providers', layout: emptyLayout(), providers: needsProviders,
        onOpen() {}, onReorder() {}, onToggle() {},
      }))));
  const errorTag = (listWithError.match(/<div[^>]*>/g) ?? []).find((tag) => tag.includes('text-meter-red'));
  assert.ok(errorTag, 'the error renders as its own line');
  assert.match(listWithError, />Could not load providers</);
  // The key dialog. Radix keeps portaled content out of static markup (as it
  // does the menus), so the contract is tested on `ApiKeyDialogView`, the
  // content the dialog renders, under a `Dialog` root whose context
  // `DialogTitle` and `DialogDescription` read; the wrapper is checked closed.
  const { ApiKeyDialog, ApiKeyDialogView } = await server.ssrLoadModule('/src/components/ApiKeyDialog.tsx');
  const { Dialog } = await server.ssrLoadModule('/src/components/ui/dialog.tsx');
  const keyProvider = { icon: 'zai', id: 'zai', name: 'Z.AI', keyLabel: 'API key', keyNote: 'coding-plan usage' };
  const keyView = (props) => renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'en' },
      React.createElement(Dialog, { open: true, onOpenChange() {} },
        React.createElement(ApiKeyDialogView, {
          checking: false, error: '', keyDraft: '', language: 'en', noAnswer: false,
          provider: keyProvider, revealed: false,
          onKeyChange() {}, onRevealToggle() {}, onCancel() {}, onSave() {},
          ...props,
        })))));
  const enabledButtons = (markup) => (markup.match(/<button[^>]*>/g) ?? []).filter((b) => !/\sdisabled=""/.test(b));
  // The approved layout: the provider's mark beside "<provider> key" and the
  // host's note (first letter raised); the field is masked, with the reveal
  // toggle inside its ring.
  const keyIdle = keyView({});
  assert.match(keyIdle, /class="key-dialog-mark"/);
  assert.match(keyIdle, />Z\.AI key</);
  assert.match(keyIdle, />Coding-plan usage</);
  assert.match(keyIdle, /<div class="key-field"><input[^>]*>.*?aria-label="Show key"/s);
  assert.match(keyIdle, /Saved in config\.toml, readable only by you\./);
  assert.match(keyIdle, /<input[^>]*type="password"/);
  assert.doesNotMatch(keyIdle, /<input[^>]*\sdisabled=""/);
  // The reveal toggle is the uniform icon button (name and tooltip on the tag).
  iconButtonTag(keyIdle, 'Show key');
  iconButtonTag(keyView({ revealed: true }), 'Hide key');
  // A form: Enter in the field submits, the same as Save.
  assert.match(keyIdle, /<form[^>]*>/);
  assert.match(keyIdle, /<button type="submit"[^>]*disabled=""/);
  // With a draft, Save is live.
  assert.doesNotMatch(keyView({ keyDraft: 'x' }), /<button type="submit"[^>]*disabled=""/);
  // Checking: the spinner and the line, the field and Save wait; only Cancel stays live.
  const keyChecking = keyView({ checking: true, keyDraft: 'x' });
  assert.match(keyChecking, /animate-spin/);
  assert.match(keyChecking, /Checking the key with Z\.AI/);
  assert.match(keyChecking, /<input[^>]*\sdisabled=""/);
  assert.equal(enabledButtons(keyChecking).length, 1);
  assert.match(keyChecking, /<button type="button"[^>]*>Cancel<\/button>/);
  // The field stands in for the key with dots, and Save says it is checking.
  assert.match(keyChecking, /<input[^>]*placeholder="\u2022+"/);
  assert.match(keyChecking, /<button type="submit"[^>]*disabled=""[^>]*>.*animate-spin.*Checking…<\/button>/s);
  // Failed: a rejection names the key, the field is empty and live again.
  const keyFailed = keyView({ error: 'HTTP 401: invalid key' });
  assert.match(keyFailed, /text-meter-red[^>]*>Z\.AI rejected this key\.</);
  assert.doesNotMatch(keyFailed, /Sign-in expired/);
  // The refusal rings the field red and reads right under it.
  assert.match(keyFailed, /<div class="key-field" data-invalid="">/);
  assert.ok(keyFailed.indexOf('key-field') < keyFailed.indexOf('rejected this key'));
  assert.doesNotMatch(keyFailed, /<input[^>]*\sdisabled=""/);
  // No answer: the fixed line, the field live.
  const keyNoAnswer = keyView({ noAnswer: true });
  assert.match(keyNoAnswer, /No answer from Z\.AI yet/);
  assert.doesNotMatch(keyNoAnswer, /<input[^>]*\sdisabled=""/);
  // Closed renders nothing.
  assert.equal(renderToStaticMarkup(React.createElement(LanguageProvider, { language: 'en' },
    React.createElement(ApiKeyDialog, { provider: null, keyCheck: null, nowMs: 1, onClose() {} }))), '');
  // A card that has no key offers "Add key", which opens the key dialog inside
  // the popover (the button carries the provider for focus to come back to);
  // nothing in it points at the TUI.
  const { ErrorRow } = await server.ssrLoadModule('/src/components/ProviderSection.tsx');
  const { explainError } = await server.ssrLoadModule('/src/model.js');
  const noKeyRow = renderToStaticMarkup(React.createElement(LanguageProvider, { language: 'en' },
    React.createElement(ErrorRow, { explained: explainError('no API key', 'zai', 'en') })));
  assert.match(noKeyRow, /<button[^>]*data-key-row="zai"[^>]*>Add key<\/button>/);
  assert.doesNotMatch(noKeyRow, /TUI/);
  // In the list, a key-only provider's alert says to use the key button
  // instead of the host's "in Settings" hint.
  const keyOnlyList = renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'en' },
      React.createElement(ProviderList, {
        cards: [], error: '', layout: emptyLayout(),
        providers: [{ id: 'zai', name: 'Z.AI', enabled: true, configured: false, env: '', signIn: 'Add an API key in Settings, then Refresh.', action: 'settings', addAccounts: [], keyLabel: 'API key', keyNote: '' }],
        onOpen() {}, onReorder() {}, onToggle() {},
      }))));
  assert.match(keyOnlyList, /aria-label="Needs an API key: add it with the key button\."/);
  assert.doesNotMatch(keyOnlyList, /in Settings, then Refresh/);
  // The sign-in sheet, as the approved design lays it out per phase. Tested on
  // `SignInDialogView` for the same reason as the key dialog.
  const { SignInDialog, SignInDialogView } = await server.ssrLoadModule('/src/components/SignInDialog.tsx');
  const signInProvider = { account: 'ChatGPT', icon: 'openai', id: 'openai', issuer: 'OpenAI', name: 'Codex' };
  const signInView = (props) => renderToStaticMarkup(React.createElement(LanguageProvider, { language: 'en' },
    React.createElement(Dialog, { open: true, onOpenChange() {} },
      React.createElement(SignInDialogView, {
        code: '', copied: false, error: '', expiresAt: 0, language: 'en', nowMs: 1_000_000, phase: 'starting',
        provider: signInProvider, reason: '',
        onCancel() {}, onCopy() {}, onOpenPage() {}, onRetry() {}, onTerminal() {},
        ...props,
      }))));
  const buttonsOf = (markup) => (markup.match(/<button[^>]*>.*?<\/button>/gs) ?? []);
  // Before the code: the mark, the title, the spinner asking the issuer; only Cancel.
  const starting = signInView({});
  assert.match(starting, /class="key-dialog-mark"/);
  assert.match(starting, />Sign in to Codex</);
  assert.match(starting, /animate-spin.*Asking OpenAI for a sign-in code…/s);
  assert.doesNotMatch(starting, /With your ChatGPT account/);
  assert.equal(buttonsOf(starting).length, 1);
  assert.match(starting, />Cancel<\/button>/);
  // The code is ready: the account under the title, the code in its box with
  // Copy, the wait with the time left, then Cancel and the primary Open page.
  const waiting = signInView({ phase: 'waiting', code: 'WXYZ-1234', expiresAt: 1_000_000 + (14 * 60 + 32) * 1000 });
  assert.match(waiting, />With your ChatGPT account</);
  assert.match(waiting, />Open the page and enter this code:</);
  assert.match(waiting, /<div class="sign-in-code"><span class="sign-in-code-text select-all">WXYZ-1234<\/span><button[^>]*>.*?Copy<\/button><\/div>/s);
  assert.match(waiting, /Waiting for you to approve it in the browser…/);
  assert.match(waiting, /aria-label="Expires in 14:32"[^>]*>14:32</);
  assert.match(waiting, /<button[^>]*data-variant="primary"[^>]*>Open page<svg/);
  assert.ok(waiting.indexOf('>Cancel<') < waiting.indexOf('>Open page<'));
  // The page address is never in the markup: the host opens it by seq.
  assert.doesNotMatch(waiting, /https?:\/\/(?!www\.w3\.org\/)/);
  assert.match(signInView({ phase: 'waiting', code: 'WXYZ-1234', copied: true }), />Copied<\/button>/);
  // Without a code there is nothing to copy.
  assert.match(signInView({ phase: 'waiting', code: '' }), /<button[^>]*disabled=""[^>]*>.*?Copy<\/button>/s);
  assert.doesNotMatch(waiting, /<button[^>]*disabled=""[^>]*>.*?Copy<\/button>/s);
  // Approved: the check, the title and the fetching line, no buttons.
  const done = signInView({ phase: 'ok' });
  assert.match(done, /class="sign-in-done"/);
  assert.match(done, />Signed in to Codex</);
  assert.match(done, />Fetching your usage…</);
  assert.equal(buttonsOf(done).length, 0);
  // Expired: the code dimmed, the reason in red, a new code as the next step.
  const expired = signInView({ phase: 'expired', code: 'WXYZ-1234' });
  assert.match(expired, /<div class="sign-in-code" data-dimmed="">/);
  assert.match(expired, /text-meter-red[^>]*>The code expired before it was approved\.</);
  assert.match(expired, /<button[^>]*data-variant="primary"[^>]*>Get a new code<\/button>/);
  // Device sign-in off: the account's setting is named, and the terminal is the way left.
  const deviceOff = signInView({ phase: 'unavailable', reason: 'device-off' });
  assert.match(deviceOff, /Device sign-in is off for this account\. Turn it on in ChatGPT(?:'|&#x27;)s security settings/);
  assert.match(deviceOff, /<button[^>]*>.*?Use the terminal<\/button>/s);
  assert.doesNotMatch(deviceOff, /data-variant="primary"/);
  assert.match(signInView({ phase: 'unavailable', reason: 'cli-missing' }), /Codex(?:'|&#x27;)s command-line tool isn(?:'|&#x27;)t installed here/);
  assert.match(signInView({ phase: 'unavailable', reason: 'custom-path' }), /config\.toml points Codex at a login file of its own/);
  // Failed: the sheet's sentence, the flow's own words under it, Try again.
  const failed = signInView({ phase: 'failed', error: 'Error logging in with device code: boom' });
  assert.match(failed, /text-meter-red[^>]*>Couldn(?:'|&#x27;)t sign in\.</);
  assert.match(failed, /text-label-2[^>]*>Error logging in with device code: boom</);
  assert.match(failed, /<button[^>]*data-variant="primary"[^>]*>Try again<\/button>/);
  assert.match(signInView({ phase: 'failed', reason: 'denied', error: 'x' }), />The sign-in was turned down in the browser\.</);
  assert.match(signInView({ phase: 'no-answer' }), /The tray hasn(?:'|&#x27;)t answered\./);
  // Closed renders nothing.
  assert.equal(renderToStaticMarkup(React.createElement(LanguageProvider, { language: 'en' },
    React.createElement(SignInDialog, { provider: null, fact: null, nowMs: 1, onClose() {} }))), '');
  // A device-code provider's card offers "Sign in", carrying the provider for focus.
  const signInRow = renderToStaticMarkup(React.createElement(LanguageProvider, { language: 'en' },
    React.createElement(ErrorRow, { explained: explainError('io error: not found', 'openai', 'en', { id: 'openai', account: 'ChatGPT', configured: false }) })));
  assert.match(signInRow, />Not signed in</);
  assert.match(signInRow, />Sign in with your ChatGPT account to track usage\.</);
  assert.match(signInRow, /<button[^>]*data-key-row="openai"[^>]*>Sign in<\/button>/);
  // In the list, a device-code provider's row wears the sign-in icon and says
  // whose account it needs; a terminal provider keeps its terminal icon.
  const deviceList = renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'en' },
      React.createElement(ProviderList, {
        cards: [], error: '', layout: emptyLayout(),
        providers: [
          { id: 'openai', name: 'Codex', enabled: true, configured: false, env: '', signIn: 'Run `codex login` in a terminal, then Refresh.', action: 'device', addAccounts: [], signInAccount: 'ChatGPT', signInIssuer: 'OpenAI' },
          { id: 'anthropic', name: 'Claude', enabled: true, configured: false, env: '', signIn: 'Run `claude` in a terminal, then Refresh.', action: 'terminal', addAccounts: [] },
        ],
        onOpen() {}, onReorder() {}, onToggle() {},
      }))));
  const listRow = (id) => {
    const start = deviceList.indexOf(`data-provider-id="${id}"`);
    const next = deviceList.indexOf('data-provider-id=', start + 1);
    return deviceList.slice(start, next === -1 ? undefined : next);
  };
  assert.match(listRow('openai'), /aria-label="Sign in with your ChatGPT account to track usage\."/);
  // The credential action's button keeps the name and the row's focus hook
  // on the icon button itself (attribute order is no contract).
  const signInTag = buttonTags(listRow('openai')).find((tag) => tag.includes('aria-label="Sign in"'));
  assert.ok(signInTag, 'the sign-in action is a button');
  assert.match(signInTag, /class="[^"]*icon-btn[^"]*"/);
  assert.match(signInTag, /data-key-row="openai"/);
  assert.doesNotMatch(listRow('openai'), /codex login/);
  assert.match(listRow('anthropic'), /aria-label="Run `claude` in a terminal, then Refresh\."/);
  // The dismissable hint card: the ✕ is the uniform icon button — a specific
  // dismissTitle is its name and tooltip, the fallback is Dismiss.
  const { HintCard } = await server.ssrLoadModule('/src/components/HintCard.tsx');
  const hintCard = (props) => renderToStaticMarkup(React.createElement(TooltipProvider, {},
    React.createElement(LanguageProvider, { language: 'en' },
      React.createElement(HintCard, {
        buttonTitle: 'Open TUI', icon: null, message: 'The TUI is the full screen.', title: 'Usage needs the TUI',
        onAction() {}, onDismiss() {}, ...props,
      }))));
  iconButtonTag(hintCard({ dismissTitle: 'Close this notice' }), 'Close this notice');
  iconButtonTag(hintCard({}), 'Dismiss');
  console.log('native dashboard: ok');
} finally {
  await server.close();
}
