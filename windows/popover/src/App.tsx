import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Footer, TopBar } from "@/components/Chrome";
import type { RowAction } from "@/components/RowMenu";
import type { RowLists } from "@/components/dnd";
import type { Layout, Screen } from "@/lib/types";
import { Customize } from "@/screens/Customize";
import { Dashboard } from "@/screens/Dashboard";
import { ProviderDetail } from "@/screens/ProviderDetail";
import { Settings } from "@/screens/Settings";
import {
  absorbPayload,
  applyCardLayout,
  applyDensity,
  applyTheme,
  emptyLayout,
  hintPending,
  emptyPayload,
  loadLayout,
  memoryStorage,
  mergeVisibleOrder,
  moveRowToList,
  parseHostPayload,
  prefsForCard,
  projectCards,
  resolvedTheme,
  saveLayout,
  sendCommand,
  setRowEnabled,
} from "./model.js";

type Direction = "back" | "forward";

/** Screens ordered as the OpenUsage pager lays them out: dashboard ← customize/provider → settings. */
const SCREEN_DEPTH: Record<Screen, number> = { dashboard: 0, customize: 1, provider: 2, settings: 3 };

function resolveStorage() {
  try {
    const ls = window.localStorage;
    ls.setItem("__aiub_t", "1");
    ls.removeItem("__aiub_t");
    return ls;
  } catch {
    return memoryStorage();
  }
}

export default function App() {
  const storageRef = useRef(resolveStorage());
  const shellRef = useRef<HTMLDivElement>(null);
  const [payload, setPayload] = useState(() => emptyPayload(""));
  const [layout, setLayout] = useState<Layout>(() => loadLayout(storageRef.current));
  const [screen, setScreen] = useState<Screen>("dashboard");
  const [direction, setDirection] = useState<Direction>("forward");
  const [providerId, setProviderId] = useState("");
  // Where the provider detail was opened from, so Back returns there: the
  // Customize list, or the dashboard header's Customize shortcut.
  const [providerFrom, setProviderFrom] = useState<Screen>("customize");
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [locked, setLocked] = useState(false);
  const [optionsOpen, setOptionsOpen] = useState(false);
  const [rowMenuOpen, setRowMenuOpen] = useState(false);
  const [resetArmed, setResetArmed] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);

  const cards = useMemo(() => (payload.hostError ? [] : projectCards(payload, nowMs)), [payload, nowMs]);
  const visible = useMemo(() => applyCardLayout(cards, layout), [cards, layout]);
  const currentCard = cards.find((card) => card.id === providerId);
  const showFooter = screen === "dashboard" || screen === "settings";

  function commit(next: Layout) {
    setLayout(next);
    saveLayout(storageRef.current, next);
  }

  function go(next: Screen) {
    setDirection(SCREEN_DEPTH[next] < SCREEN_DEPTH[screen] ? "back" : "forward");
    setScreen(next);
    setResetArmed(false);
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }

  function goBack() {
    if (screen === "provider") go(providerFrom === "dashboard" ? "dashboard" : "customize");
    else go("dashboard");
  }

  useEffect(() => {
    applyTheme(layout.theme);
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => applyTheme(layout.theme);
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, [layout.theme]);

  useEffect(() => {
    applyDensity(layout.density);
  }, [layout.density]);

  useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    window.__AIUB_APPLY__ = (raw) => {
      const next = parseHostPayload(raw);
      setPayload(next);
      setLayout((current) => {
        const synced = absorbPayload(current, next.entries);
        if (synced !== current) saveLayout(storageRef.current, synced);
        return synced;
      });
    };
    window.__AIUB_VISIBLE__ = (visible) => {
      if (visible) return;
      // Closing the popover resets navigation: back to the dashboard, scrolled to the top,
      // menus closed (OpenUsage "Closing").
      setOptionsOpen(false);
      setRowMenuOpen(false);
      setResetArmed(false);
      setScreen("dashboard");
      if (scrollRef.current) scrollRef.current.scrollTop = 0;
    };
    window.__AIUB_LOCKCLICKS__ = (ms) => {
      const hold = Math.max(0, Number(ms) || 0);
      setLocked(true);
      document.documentElement.style.pointerEvents = "none";
      window.setTimeout(() => {
        setLocked(false);
        document.documentElement.style.pointerEvents = "";
      }, hold);
    };
    sendCommand("ready");
    return () => {
      delete window.__AIUB_APPLY__;
      delete window.__AIUB_LOCKCLICKS__;
      delete window.__AIUB_VISIBLE__;
    };
  }, []);

  // The panel follows its content (PanelHeightCoordinator): report the intrinsic height of the
  // shell — chrome plus unscrolled content — and let the host clamp it to the work area. A
  // macrotask, not requestAnimationFrame: the WebView stops painting while the popover is
  // hidden, and a payload that lands then must still size the window before it is shown.
  useLayoutEffect(() => {
    const shell = shellRef.current;
    if (!shell) return;
    let frame = 0;
    let last = -1;
    const report = () => {
      frame = 0;
      const content = shell.querySelector<HTMLElement>("[data-scroll-content]");
      let height = 0;
      for (const child of Array.from(shell.children)) {
        const el = child as HTMLElement;
        height += el.dataset.scroll !== undefined && content ? content.offsetHeight : el.offsetHeight;
      }
      height = Math.ceil(height);
      if (height <= 0 || height === last) return;
      last = height;
      sendCommand("resize", { height, theme: resolvedTheme(layout.theme) });
    };
    const schedule = () => {
      if (frame === 0) frame = window.setTimeout(report, 0);
    };
    const observer = new ResizeObserver(schedule);
    observer.observe(shell);
    for (const child of Array.from(shell.children)) observer.observe(child);
    const content = shell.querySelector<HTMLElement>("[data-scroll-content]");
    if (content) observer.observe(content);
    schedule();
    return () => {
      observer.disconnect();
      if (frame !== 0) window.clearTimeout(frame);
    };
  }, [screen, layout.theme, layout.density]);

  function onKeyDown(event: KeyboardEvent) {
    if (locked || event.defaultPrevented || optionsOpen || rowMenuOpen) return;
    // The shortcut recorder owns every key while it records (Escape cancels it, not the screen).
    if (document.activeElement?.closest("[data-recording]")) return;
    // Controls own Enter/Escape: switches, pickers, menus, sortable handles.
    const target = event.target instanceof Element ? event.target : null;
    const onControl = !!target?.closest(
      'button, input, select, textarea, [role="button"], [role="option"], [role="listbox"], [role="menu"], [role="menuitem"]',
    );
    if (event.key === "Escape") {
      if (onControl && screen !== "dashboard" && target?.closest('[role="listbox"], [role="menu"]')) return;
      if (screen !== "dashboard") goBack();
      else sendCommand("close");
      return;
    }
    if (event.key === "Enter" && !onControl) {
      if (screen === "dashboard") go("customize");
      else goBack();
    }
  }

  useEffect(() => {
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  });

  function toggleShowAs() {
    commit({ ...layout, showAs: layout.showAs === "used" ? "left" : "used" });
  }

  function toggleResetTimes() {
    commit({ ...layout, resetTimes: layout.resetTimes === "exact" ? "countdown" : "exact" });
  }

  // Two clicks within three seconds; a native confirm() would steal focus and the
  // popover hides itself on focus loss. Like OpenUsage's Reset All, it also re-runs
  // provider detection so the layout starts from the tools on this machine.
  function resetAll() {
    if (!resetArmed) {
      setResetArmed(true);
      window.setTimeout(() => setResetArmed(false), 3000);
      return;
    }
    setResetArmed(false);
    commit(
      absorbPayload(
        {
          ...emptyLayout(),
          alwaysShowPace: layout.alwaysShowPace,
          density: layout.density,
          resetTimes: layout.resetTimes,
          showAs: layout.showAs,
          theme: layout.theme,
          timeFormat: layout.timeFormat,
        },
        payload.entries,
      ),
    );
    sendCommand("detect");
  }

  function resetProviderRows(id: string) {
    if (!id) return;
    const rows = { ...layout.rows };
    delete rows[id];
    const collapsed = { ...layout.collapsed };
    delete collapsed[id];
    commit({ ...layout, collapsed, rows });
  }

  function openProvider(id: string, from: Screen) {
    setProviderId(id);
    setProviderFrom(from);
    go("provider");
  }

  function reorderRows(lists: RowLists) {
    if (!currentCard) return;
    const prevOff = prefsForCard(currentCard, layout).off || {};
    const off: Record<string, boolean> = {};
    const demand: string[] = [];
    for (const key of lists.demand) {
      if (prevOff[key]) off[key] = true;
      else demand.push(key);
    }
    commit({ ...layout, rows: { ...layout.rows, [providerId]: { always: lists.always, demand, off } } });
  }

  // Row context menu. Hide / Always show / Show on demand rewrite that provider's row prefs the
  // same way the Customize screen does; Refresh and Customize are provider-level shortcuts.
  function onRowAction(id: string, key: string, action: RowAction) {
    const card = cards.find((item) => item.id === id);
    if (!card) return;
    if (action === "refresh") {
      sendCommand("refresh-entry", { id });
      return;
    }
    if (action === "customize") {
      openProvider(id, "dashboard");
      return;
    }
    const prefs = prefsForCard(card, layout);
    const next = action === "hide" ? setRowEnabled(prefs, key, false) : moveRowToList(prefs, key, action);
    commit({ ...layout, rows: { ...layout.rows, [id]: next } });
  }

  const title =
    screen === "customize" ? "Customize" : screen === "settings" ? "Settings" : currentCard?.title || "Provider";

  return (
    <div ref={shellRef} className="flex h-full flex-col bg-background text-foreground">
      {screen !== "dashboard" ? (
        <TopBar
          resetArmed={resetArmed}
          title={title}
          resetLabel={screen === "customize" ? "Reset All Customization" : screen === "provider" ? `Reset ${title}` : undefined}
          onBack={goBack}
          onReset={screen === "customize" ? resetAll : screen === "provider" ? () => resetProviderRows(providerId) : undefined}
        />
      ) : null}
      <div ref={scrollRef} data-scroll className="min-h-0 flex-1 overflow-y-auto">
        <div
          key={screen}
          data-direction={direction}
          data-scroll-content
          className="screen-enter px-[var(--panel-pad)] pb-3 pt-[var(--content-top)]"
        >
          {screen === "dashboard" ? (
            <Dashboard
              cards={cards}
              hint={hintPending(layout)}
              layout={layout}
              nowMs={nowMs}
              payload={payload}
              visible={visible}
              onCustomizeProvider={(id) => openProvider(id, "dashboard")}
              onDismissHint={() => commit({ ...layout, hintDismissed: true })}
              onOpenCustomize={() => go("customize")}
              onResetProvider={resetProviderRows}
              onReorder={(ids) => commit({ ...layout, cardOrder: mergeVisibleOrder(layout.cardOrder, ids) })}
              onRowAction={onRowAction}
              onRowMenuOpenChange={setRowMenuOpen}
              onToggleCollapse={(id) => {
                const collapsed = { ...layout.collapsed };
                if (collapsed[id]) delete collapsed[id];
                else collapsed[id] = true;
                commit({ ...layout, collapsed });
              }}
              onToggleResetTimes={toggleResetTimes}
              onToggleShowAs={toggleShowAs}
            />
          ) : null}
          {screen === "customize" ? (
            <Customize
              cards={cards}
              layout={layout}
              onOpen={(id) => openProvider(id, "customize")}
              onOpenSettings={() => go("settings")}
              onReorder={(ids) => commit({ ...layout, cardOrder: mergeVisibleOrder(layout.cardOrder, ids) })}
              onToggle={(id, on) => {
                const hidden = { ...layout.hidden };
                if (on) delete hidden[id];
                else hidden[id] = true;
                commit({ ...layout, hidden });
              }}
            />
          ) : null}
          {screen === "provider" ? (
            <ProviderDetail
              card={currentCard}
              layout={layout}
              onReorderRows={reorderRows}
              onToggleRow={(key, on) => {
                if (!currentCard) return;
                commit({
                  ...layout,
                  rows: { ...layout.rows, [providerId]: setRowEnabled(prefsForCard(currentCard, layout), key, on) },
                });
              }}
            />
          ) : null}
          {screen === "settings" ? (
            <Settings
              layout={layout}
              payload={payload}
              onAlwaysShowPace={(alwaysShowPace) => commit({ ...layout, alwaysShowPace })}
              onDensity={(density) => commit({ ...layout, density })}
              onOpenCustomize={() => go("customize")}
              onResetTimes={(resetTimes) => commit({ ...layout, resetTimes })}
              onShowAs={(showAs) => commit({ ...layout, showAs })}
              onTheme={(theme) => commit({ ...layout, theme })}
              onTimeFormat={(timeFormat) => commit({ ...layout, timeFormat })}
            />
          ) : null}
        </div>
      </div>
      {showFooter ? (
        <Footer
          locked={locked}
          nowMs={nowMs}
          optionsOpen={optionsOpen}
          payload={payload}
          onOpenCustomize={() => {
            setOptionsOpen(false);
            go("customize");
          }}
          onOpenSettings={() => {
            setOptionsOpen(false);
            go("settings");
          }}
          onOptionsOpenChange={setOptionsOpen}
        />
      ) : null}
    </div>
  );
}
