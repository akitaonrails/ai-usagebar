import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Footer, TopBar } from "@/components/Chrome";
import type { RowAction } from "@/components/RowMenu";
import type { RowLists } from "@/components/dnd";
import type { Layout, Screen } from "@/lib/types";
import { LanguageProvider, translate } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { About } from "@/screens/About";
import { Customize } from "@/screens/Customize";
import { Dashboard } from "@/screens/Dashboard";
import { MacDashboard } from "@/screens/MacDashboard";
import { ProviderDetail } from "@/screens/ProviderDetail";
import { Settings, type SettingsTab } from "@/screens/Settings";
import {
  absorbPayload,
  applyCardLayout,
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
  seedStars,
  sendCommand,
  setRowEnabled,
  stripCommand,
  toggleStar,
} from "./model.js";
import { measurePanelHeight } from "./panel-size.js";

type Direction = "back" | "forward";

/** Screens ordered as the OpenUsage pager lays them out: dashboard ← customize/provider → settings. */
const SCREEN_DEPTH: Record<Screen, number> = { dashboard: 0, customize: 1, provider: 2, settings: 3, about: 4 };

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
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("general");
  const [direction, setDirection] = useState<Direction>("forward");
  const [providerId, setProviderId] = useState("");
  // Where the provider detail was opened from, so Back returns there: the
  // Customize list, or the dashboard header's Customize shortcut.
  const [providerFrom, setProviderFrom] = useState<Screen>("customize");
  const [aboutFrom, setAboutFrom] = useState<Screen>("dashboard");
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [locked, setLocked] = useState(false);
  const [optionsOpen, setOptionsOpen] = useState(false);
  const [rowMenuOpen, setRowMenuOpen] = useState(false);
  const [resetArmed, setResetArmed] = useState(false);
  const [starError, setStarError] = useState("");
  const [popoverVisible, setPopoverVisible] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);

  const cards = useMemo(() => (payload.hostError ? [] : projectCards(payload, nowMs)), [payload, nowMs]);
  const visible = useMemo(() => applyCardLayout(cards, layout), [cards, layout]);
  const currentCard = cards.find((card) => card.id === providerId);

  function commit(next: Layout) {
    setLayout(next);
    saveLayout(storageRef.current, next);
    sendCommand("strip", stripCommand(next, cards));
  }

  function go(next: Screen) {
    if (next === "customize" && payload.os === "macos") {
      setSettingsTab("providers");
      next = "settings";
    }
    setDirection(SCREEN_DEPTH[next] < SCREEN_DEPTH[screen] ? "back" : "forward");
    setScreen(next);
    setResetArmed(false);
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }

  function changeSettingsTab(next: SettingsTab) {
    setSettingsTab(next);
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }

  function goBack() {
    if (screen === "about") {
      go(aboutFrom === "about" ? "dashboard" : aboutFrom);
      return;
    }
    if (screen === "provider") go(providerFrom);
    else go("dashboard");
  }

  function openAbout(check: boolean) {
    setOptionsOpen(false);
    if (screen !== "about") setAboutFrom(screen);
    if (check) sendCommand("check-update");
    go("about");
  }

  useEffect(() => {
    applyTheme(layout.theme);
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => applyTheme(layout.theme);
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, [layout.theme]);

  useEffect(() => {
    document.documentElement.lang = layout.language;
  }, [layout.language]);

  useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    window.__AIUB_APPLY__ = (raw) => {
      const next = parseHostPayload(raw);
      setPayload(next);
      setLayout((current) => {
        const cards = projectCards(next, Date.now());
        const synced = seedStars(absorbPayload(current, next.entries), cards);
        if (synced !== current) saveLayout(storageRef.current, synced);
        sendCommand("strip", stripCommand(synced, cards));
        return synced;
      });
    };
    window.__AIUB_VISIBLE__ = (visible) => {
      // Visibility changes are also sizing boundaries. ResizeObserver callbacks
      // can be suspended while WebView2 is hidden, so force a fresh measurement
      // as soon as the native host opens the popover again.
      setPopoverVisible(visible);
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
      const measured = measurePanelHeight(shell);
      // Same floor as tray MIN_POPOVER_HEIGHT: keep room for the Options menu
      // (side=top from the footer, nine rows) so Radix does not scroll the list.
      const height = measured > 0 ? Math.max(measured, 360) : measured;
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
  }, [screen, payload, layout, popoverVisible]);

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
          language: layout.language,
          alwaysShowPace: layout.alwaysShowPace,
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
    commit({
      ...layout,
      rows: { ...layout.rows, [providerId]: { always: lists.always, demand: lists.demand, off: prevOff } },
    });
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
    if (action === "star") {
      const result = toggleStar(layout.stars, id, key);
      if (result.error) {
        setStarError(result.error);
        window.setTimeout(() => setStarError(""), 2200);
        return;
      }
      commit({ ...layout, stars: result.stars });
      return;
    }
    const prefs = prefsForCard(card, layout);
    const next = action === "hide" ? setRowEnabled(prefs, key, false) : moveRowToList(prefs, key, action);
    commit({ ...layout, rows: { ...layout.rows, [id]: next } });
  }

  const title =
    screen === "customize"
      ? translate(layout.language, "Customize")
      : screen === "settings"
        ? translate(layout.language, "Settings")
        : screen === "about"
          ? translate(layout.language, "About")
          : currentCard?.title || "Provider";

  return (
    <LanguageProvider language={layout.language}>
    <div
      ref={shellRef}
      className={cn(
        "flex h-full flex-col overflow-hidden rounded-[13px] bg-background text-foreground",
        payload.os === "macos" && "mac-panel",
      )}
    >
      {screen !== "dashboard" ? (
        <TopBar
          resetArmed={resetArmed}
          title={title}
          resetLabel={screen === "customize" ? translate(layout.language, "Reset All Customization") : screen === "provider" ? `${translate(layout.language, "Reset")} ${title}` : undefined}
          onBack={goBack}
          onReset={screen === "customize" ? resetAll : screen === "provider" ? () => resetProviderRows(providerId) : undefined}
        />
      ) : null}
      <div ref={scrollRef} data-scroll className="min-h-0 flex-1 overflow-y-auto">
        <div
          key={screen}
          data-direction={direction}
          data-scroll-content
          className={cn(
            "screen-enter px-[var(--panel-pad)] pb-0",
            screen === "dashboard" ? "pt-[var(--panel-pad)]" : "pt-0",
          )}
        >
          {screen === "dashboard" ? (
            payload.os === "macos" ? (
              <MacDashboard
                cards={visible}
                layout={layout}
                nowMs={nowMs}
                payload={payload}
                onOpenCustomize={() => go("customize")}
                onOpenSettings={() => go("settings")}
              />
            ) : (
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
              onToggleShowAs={toggleShowAs}
              />
            )
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
              starError={starError}
              onReorderRows={reorderRows}
              onToggleStar={(key) => {
                if (!currentCard) return;
                const result = toggleStar(layout.stars, currentCard.id, key);
                if (result.error) {
                  setStarError(result.error);
                  window.setTimeout(() => setStarError(""), 2200);
                  return;
                }
                commit({ ...layout, stars: result.stars });
              }}
              onToggleRow={(key, on) => {
                if (!currentCard) return;
                commit({
                  ...layout,
                  rows: { ...layout.rows, [providerId]: setRowEnabled(prefsForCard(currentCard, layout), key, on) },
                });
              }}
            />
          ) : null}
          {screen === "about" ? <About nowMs={nowMs} payload={payload} /> : null}
          {screen === "settings" ? (
            <Settings
              tab={settingsTab}
              onTabChange={changeSettingsTab}
              cards={cards}
              layout={layout}
              nowMs={nowMs}
              payload={payload}
              onAlwaysShowPace={(alwaysShowPace) => commit({ ...layout, alwaysShowPace })}
              onUsageGoal={(usageGoal) => commit({ ...layout, usageGoal })}
              onLanguage={(language) => commit({ ...layout, language })}
              onOpenCustomize={() => go("customize")}
              onOpenProvider={(id) => openProvider(id, "settings")}
              onReorderProviders={(ids) => commit({ ...layout, cardOrder: mergeVisibleOrder(layout.cardOrder, ids) })}
              onToggleProvider={(id, on) => {
                const hidden = { ...layout.hidden };
                if (on) delete hidden[id];
                else hidden[id] = true;
                commit({ ...layout, hidden });
              }}
              onResetCustomization={resetAll}
              resetArmed={resetArmed}
              onResetTimes={(resetTimes) => commit({ ...layout, resetTimes })}
              onShowAs={(showAs) => commit({ ...layout, showAs })}
              onTheme={(theme) => commit({ ...layout, theme })}
              onTimeFormat={(timeFormat) => commit({ ...layout, timeFormat })}
            />
          ) : null}
        </div>
      </div>
      <Footer
        locked={locked}
        nowMs={nowMs}
        optionsOpen={optionsOpen}
        payload={payload}
        updatePending={payload.update !== null}
        onCheckUpdates={() => openAbout(true)}
        onOpenAbout={() => openAbout(false)}
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
    </div>
    </LanguageProvider>
  );
}
