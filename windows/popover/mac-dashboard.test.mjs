import assert from 'node:assert/strict';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

process.env.TZ = 'UTC';
const server = await createServer({ server: { middlewareMode: true, hmr: false, ws: false }, appType: 'custom' });

try {
  const { MacDashboard } = await server.ssrLoadModule('/src/screens/MacDashboard.tsx');
  const { LanguageProvider } = await server.ssrLoadModule('/src/lib/i18n.tsx');
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
  console.log('macOS dashboard reset display: ok');
} finally {
  await server.close();
}
