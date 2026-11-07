import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Footer, TopBar } from "@/components/Chrome";
import type { RowAction } from "@/components/RowMenu";
import type { RowLists } from "@/components/dnd";
import { ApiKeyDialog, type KeyDialogProvider } from "@/components/ApiKeyDialog";
import { SignInDialog, type SignInProvider } from "@/components/SignInDialog";
import { UpdateDialog } from "@/components/UpdateDialog";
import type { Layout, Screen } from "@/lib/types";
import { LanguageProvider } from "@/lib/i18n";
import { cardImagePng, createCopyRegistry, findCardElement, type CopyRegistry } from "@/lib/cardImage";
import { PopoverActionsProvider, providerFocusTarget, type PopoverActions } from "@/lib/popoverActions";
import { m } from "@/paraglide/messages.js";
import { cn } from "@/lib/utils";
import { About } from "@/screens/About";
import { Customize } from "@/screens/Customize";
import { Dashboard } from "@/screens/Dashboard";
import { NativeDashboard } from "@/screens/NativeDashboard";
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
  menuAction,
  mergeVisibleOrder,
  moveRowToList,
  optionsMenuLabels,
  parseHostPayload,
  prefsForCard,
  projectCards,
  providerGroups,
  providerIconId,
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

/** How long the footer's copy-image notice stays up before it fades on its own. */
const COPY_NOTICE_MS = 2500;

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
  const native = layout.popoverStyle === "native";
  const [screen, setScreen] = useState<Screen>("dashboard");
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("general");
  const [direction, setDirection] = useState<Direction>("forward");
  const [providerId, setProviderId] = useState("");
  // Where the provider detail was opened from, so Back returns there: the
  // Customize list, or the dashboard header's Customize shortcut.
  const [providerFrom, setProviderFrom] = useState<Screen>("customize");
  const [aboutFrom, setAboutFrom] = useState<Screen>("dashboard");
  const [updateDialogOpen, setUpdateDialogOpen] = useState(false);
  // When the user last asked for a check (for the no-answer timeout, on this clock) and the
  // host's last check stamp at that moment (the answer is any newer stamp, on the host's clock).
  const [updateCheck, setUpdateCheck] = useState({ at: 0, baseline: 0 });
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [locked, setLocked] = useState(false);
  const [optionsOpen, setOptionsOpen] = useState(false);
  const [rowMenuOpen, setRowMenuOpen] = useState(false);
  const [resetArmed, setResetArmed] = useState(false);
  // The key dialog, opened from the provider list's key icon or from a card's
  // "No API key" row; focus returns to whichever control opened it.
  const [keyProvider, setKeyProvider] = useState<KeyDialogProvider | null>(null);
  const keyTrigger = useRef<HTMLElement | null>(null);
  const closeKeyDialog = useCallback(() => setKeyProvider(null), []);
  // An accepted key can remove the control that opened the dialog (the row or
  // the card no longer needs its credential): then focus lands on that
  // provider's switch in the list, or on its card's header action.
  const keyFocus = useCallback(() => {
    const trigger = keyTrigger.current;
    if (trigger && trigger.isConnected) return trigger;
    const id = trigger?.dataset.keyRow;
    return id ? providerFocusTarget(id) : null;
  }, []);
  // The sign-in sheet for a device-code provider, opened from the list's
  // sign-in icon or a card's "Sign in"; focus returns the same way.
  const [signInProvider, setSignInProvider] = useState<SignInProvider | null>(null);
  const signInTrigger = useRef<HTMLElement | null>(null);
  const closeSignIn = useCallback(() => setSignInProvider(null), []);
  const signInFocus = useCallback(() => {
    const trigger = signInTrigger.current;
    if (trigger && trigger.isConnected) return trigger;
    const id = trigger?.dataset.keyRow;
    return id ? providerFocusTarget(id) : null;
  }, []);
  const [starError, setStarError] = useState("");
  // The pending card image copies, keyed by the id sent with each command (contract v2):
  // the host answers window.__AIUB_IMAGE_COPIED__(id, ok) for the id it received, so the
  // association lives in the id, not in the order the IPCs arrived. A copy settles
  // exactly once — success, failure, timeout or pre-send failure — and its id leaves the
  // pending set, so a late answer for it is ignored.
  const [copyNotice, setCopyNotice] = useState<{ failed: boolean; text: string } | null>(null);
  const copyRegistryRef = useRef<CopyRegistry | null>(null);
  if (copyRegistryRef.current === null) copyRegistryRef.current = createCopyRegistry();
  const copyNoticeTimerRef = useRef(0);
  const [popoverVisible, setPopoverVisible] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);

  const cards = useMemo(() => (payload.hostError ? [] : projectCards(payload, nowMs, layout.language)), [payload, nowMs, layout.language]);
  const visible = useMemo(() => applyCardLayout(cards, layout), [cards, layout]);
  const currentCard = cards.find((card) => card.id === providerId);
  // The provider's accounts for the detail screen's tabs: the group that holds the
  // opened card, so a provider with several accounts names itself and lists them.
  const currentGroup = providerGroups(cards).find((group) => group.cards.some((card) => card.id === providerId));
  const currentProviderName = currentGroup
    ? payload.providers.find((provider) => provider.id === currentGroup.id)?.name ?? currentGroup.cards[0].title
    : "";

  function selectAccount(providerId: string, cardId: string) {
    commit({ ...layout, accountTab: { ...layout.accountTab, [providerId]: cardId } });
  }

  function commit(next: Layout) {
    setLayout(next);
    saveLayout(storageRef.current, next);
    sendCommand("strip", stripCommand(next, cards));
  }

  function go(next: Screen) {
    if (next === "customize" && native) {
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

  function openAbout() {
    setOptionsOpen(false);
    if (screen !== "about") setAboutFrom(screen);
    go("about");
  }

  /** Check in place: the dialog opens over whatever screen is showing. */
  function checkForUpdates() {
    setOptionsOpen(false);
    requestCheck();
    setUpdateDialogOpen(true);
  }

  function requestCheck() {
    setUpdateCheck({ at: Date.now(), baseline: payload.updateCheckedAt });
    sendCommand("check-update");
  }

  // Native tray menu entry point: the host posts one of the whitelisted actions
  // and we open the matching popover screen, exactly like the Options items do.
  function onMenuAction(action: string) {
    switch (menuAction(action)) {
      case "customize":
        setOptionsOpen(false);
        go("customize");
        break;
      case "settings":
        setOptionsOpen(false);
        go("settings");
        break;
      case "about":
        openAbout();
        break;
      case "check-updates":
        checkForUpdates();
        break;
      default:
        break;
    }
  }

  // The hook is installed once, so it calls onMenuAction through a ref refreshed on every
  // render — never opening a screen with a stale `screen`/`payload`.
  const menuActionRef = useRef(onMenuAction);
  menuActionRef.current = onMenuAction;

  useEffect(() => {
    // The host tints its native backdrop by the page's theme, so it hears about every change,
    // including a System theme flipping with the OS, which moves no height to report.
    const apply = () => {
      applyTheme(layout.theme);
      sendCommand("resize", { theme: resolvedTheme(layout.theme), style: layout.popoverStyle });
    };
    apply();
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [layout.theme, layout.popoverStyle]);

  useEffect(() => {
    document.documentElement.lang = layout.language;
  }, [layout.language]);

  useEffect(() => {
    const style = document.documentElement.style;
    if (payload.accent) {
      style.setProperty("--system-accent-light", payload.accent.light);
      style.setProperty("--system-accent-dark", payload.accent.dark);
    } else {
      style.removeProperty("--system-accent-light");
      style.removeProperty("--system-accent-dark");
    }
    return () => {
      style.removeProperty("--system-accent-light");
      style.removeProperty("--system-accent-dark");
    };
  }, [payload.accent]);

  useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    window.__AIUB_APPLY__ = (raw) => {
      const next = parseHostPayload(raw);
      setPayload(next);
      setLayout((current) => {
        const cards = projectCards(next, Date.now(), current.language);
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
      // menus closed (OpenUsage "Closing"), the copy notice gone.
      setOptionsOpen(false);
      setRowMenuOpen(false);
      setUpdateDialogOpen(false);
      setResetArmed(false);
      setScreen("dashboard");
      setCopyNotice(null);
      if (copyNoticeTimerRef.current) window.clearTimeout(copyNoticeTimerRef.current);
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
    // The tray host calls this for the native menu items that open a popover
    // screen (Customize / Settings / About / Check for Updates).
    window.__AIUB_MENU_ACTION__ = (action) => menuActionRef.current(String(action));
    // The tray host's answer for one copy-image command: __AIUB_IMAGE_COPIED__(id, ok)
    // with the id the host received (null when it had none to match). Only a still-pending
    // id is applied; unknown, expired and null ids are ignored.
    window.__AIUB_IMAGE_COPIED__ = (id, ok) => copyRegistryRef.current?.answer(id, ok === true);
    sendCommand("ready");
    return () => {
      delete window.__AIUB_APPLY__;
      delete window.__AIUB_LOCKCLICKS__;
      delete window.__AIUB_VISIBLE__;
      delete window.__AIUB_MENU_ACTION__;
      delete window.__AIUB_IMAGE_COPIED__;
      if (copyNoticeTimerRef.current) window.clearTimeout(copyNoticeTimerRef.current);
    };
  }, []);

  // The native tray menu must match the popover's language: publish the menu's
  // labels on mount and whenever the language changes.
  useEffect(() => {
    sendCommand("menu-labels", optionsMenuLabels(layout.language));
  }, [layout.language]);

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
      sendCommand("resize", { height, theme: resolvedTheme(layout.theme), style: layout.popoverStyle });
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
    // An open menu or dialog owns Escape/Enter: Escape closes it, not the screen or the popover.
    if (locked || event.defaultPrevented || optionsOpen || rowMenuOpen || updateDialogOpen) return;
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

  // Copy one provider's card as an image: reserve the copy's id, rasterize the card in the
  // page (self-contained, so the canvas stays untainted), hand the base64 and the id to
  // the host over IPC, and let the host write the clipboard natively and answer for the id
  // it received. The id keeps the association: a slow raster that outlives its timeout may
  // dispatch after a newer copy, and a lost answer cannot settle another copy. Any failure
  // before the host is asked (the card gone, a raster error — a SecurityError among
  // others, the send itself throwing) settles the id with a failure, and the host never
  // hears a command that was not sent.
  function copyCardAsImage(cardId: string) {
    const registry = copyRegistryRef.current;
    // The double-click lock is held from the click until the copy settles (success,
    // failure, timeout or pre-send failure): after a timeout, a new copy is allowed.
    if (!registry || registry.isBusy()) return;
    const copy = registry.begin((ok) => showCopyNotice(ok));
    void (async () => {
      try {
        // The id is never interpolated into a selector (it may carry a double quote);
        // a missing card is a pre-send failure, not an unhandled throw.
        const section = findCardElement(shellRef.current, cardId);
        if (!section) throw new Error("the card is not on screen");
        const png = await cardImagePng(section);
        sendCommand("copy-image", { png, id: copy.id });
      } catch {
        // Settling with false removes the id from the pending set: nothing keeps waiting
        // for an answer to a command the host never received.
        copy.settle(false);
      }
    })();
  }

  /** The footer's short copy-image notice: success in the footer's tone, failure in the meter's red. */
  function showCopyNotice(ok: boolean) {
    if (copyNoticeTimerRef.current) window.clearTimeout(copyNoticeTimerRef.current);
    copyNoticeTimerRef.current = window.setTimeout(() => {
      copyNoticeTimerRef.current = 0;
      setCopyNotice(null);
    }, COPY_NOTICE_MS);
    setCopyNotice(
      ok
        ? { failed: false, text: m.copied_to_clipboard({}, { locale: layout.language }) }
        : { failed: true, text: m.copy_image_failed({}, { locale: layout.language }) },
    );
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
  // same way the Customize screen does; Customize is a provider-level shortcut.
  function onRowAction(id: string, key: string, action: RowAction) {
    const card = cards.find((item) => item.id === id);
    if (!card) return;
    if (action === "copy-image") {
      copyCardAsImage(id);
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
      ? m.customize({}, { locale: layout.language })
      : screen === "settings"
        ? m.settings({}, { locale: layout.language })
        : screen === "about"
          ? m.about({}, { locale: layout.language })
          : screen === "provider"
            ? (currentGroup && currentGroup.cards.length > 1 ? currentProviderName : currentCard?.title) || m.provider({}, { locale: layout.language })
            : currentCard?.title || m.provider({}, { locale: layout.language });

  const popoverActions: PopoverActions = {
    addKey(providerId, trigger) {
      const provider = payload.providers.find((item) => item.id === providerId);
      // Only a key-only provider has a key to take here; anything else keeps the TUI.
      if (!provider || !provider.keyLabel) {
        sendCommand("open-tui");
        return;
      }
      keyTrigger.current = trigger;
      setKeyProvider({
        icon: providerIconId(provider.id),
        id: provider.id,
        keyLabel: provider.keyLabel,
        keyNote: provider.keyNote || "",
        name: provider.name,
      });
    },
    deviceProvider(cardId) {
      // Only the provider's own card: an `@account` card has a login of its
      // own, which the provider's device flow does not write.
      const provider = payload.providers.find((item) => item.id === cardId);
      if (!provider || provider.action !== "device" || !provider.signInAccount) return null;
      return { account: provider.signInAccount, configured: provider.configured, id: provider.id };
    },
    openCustomize: () => go("customize"),
    signIn(providerId, trigger) {
      const provider = payload.providers.find((item) => item.id === providerId);
      // A provider without a device flow keeps its terminal sign-in.
      if (!provider || provider.action !== "device") {
        sendCommand("sign-in-provider", { id: providerId });
        return;
      }
      signInTrigger.current = trigger;
      setSignInProvider({
        account: provider.signInAccount || provider.name,
        icon: providerIconId(provider.id),
        id: provider.id,
        issuer: provider.signInIssuer || provider.name,
        name: provider.name,
      });
    },
  };

  return (
    <LanguageProvider language={layout.language}>
    <PopoverActionsProvider value={popoverActions}>
    <div
      ref={shellRef}
      data-os={payload.os}
      className={cn(
        "flex h-full flex-col overflow-hidden rounded-[var(--window-radius)] bg-background text-foreground",
        native && "native-panel",
      )}
    >
      {screen !== "dashboard" ? (
        <TopBar
          resetArmed={resetArmed}
          title={title}
          resetLabel={screen === "customize" ? m.reset_all_customization({}, { locale: layout.language }) : screen === "provider" ? `${m.reset({}, { locale: layout.language })} ${title}` : undefined}
          onBack={goBack}
          onDetect={screen === "customize" && !native ? () => sendCommand("detect") : undefined}
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
            // Native began as the macOS dashboard and now serves every OS.
            native ? (
              <NativeDashboard
                cards={visible}
                hint={hintPending(layout)}
                layout={layout}
                nowMs={nowMs}
                payload={payload}
                onCustomizeProvider={(id) => openProvider(id, "dashboard")}
                onCopyImage={copyCardAsImage}
                onDismissHint={() => commit({ ...layout, hintDismissed: true })}
                onOpenCustomize={() => go("customize")}
                onOpenSettings={() => go("settings")}
                onRowAction={onRowAction}
                onRowMenuOpenChange={setRowMenuOpen}
                onSelectAccount={selectAccount}
                onSwitchAccount={(vendor, label) => sendCommand("switch-account", { vendor, label })}
                onToggleCollapse={(id) => {
                  const collapsed = { ...layout.collapsed };
                  if (collapsed[id]) delete collapsed[id];
                  else collapsed[id] = true;
                  commit({ ...layout, collapsed });
                }}
                onToggleShowAs={toggleShowAs}
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
              onCopyImage={copyCardAsImage}
              onDismissHint={() => commit({ ...layout, hintDismissed: true })}
              onOpenCustomize={() => go("customize")}
              onReorder={(ids) => commit({ ...layout, cardOrder: mergeVisibleOrder(layout.cardOrder, ids) })}
              onRowAction={onRowAction}
              onRowMenuOpenChange={setRowMenuOpen}
              onSelectAccount={selectAccount}
              onSwitchAccount={(vendor, label) => sendCommand("switch-account", { vendor, label })}
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
              payload={payload}
              onOpen={(id) => openProvider(id, "customize")}
              onOpenSettings={() => go("settings")}
              onReorder={(ids) => commit({ ...layout, cardOrder: mergeVisibleOrder(layout.cardOrder, ids) })}
              onToggle={(id, on) => sendCommand("set-provider-enabled", { id, value: on })}
            />
          ) : null}
          {screen === "provider" ? (
            <ProviderDetail
              card={currentCard}
              cards={currentGroup ? currentGroup.cards : undefined}
              providerName={currentGroup ? currentProviderName : undefined}
              layout={layout}
              starError={starError}
              onSelectAccount={(cardId) => setProviderId(cardId)}
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
          {screen === "about" ? <About payload={payload} /> : null}
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
              onCheckUpdates={checkForUpdates}
              onOpenCustomize={() => go("customize")}
              onOpenProvider={(id) => openProvider(id, "settings")}
              onReorderProviders={(ids) => commit({ ...layout, cardOrder: mergeVisibleOrder(layout.cardOrder, ids) })}
              onToggleProvider={(id, on) => sendCommand("set-provider-enabled", { id, value: on })}
              onResetCustomization={resetAll}
              resetArmed={resetArmed}
              onResetTimes={(resetTimes) => commit({ ...layout, resetTimes })}
              onShowAs={(showAs) => commit({ ...layout, showAs })}
              onTheme={(theme) => commit({ ...layout, theme })}
              onTimeFormat={(timeFormat) => commit({ ...layout, timeFormat })}
              onPopoverStyle={(popoverStyle) => commit({ ...layout, popoverStyle })}
            />
          ) : null}
        </div>
      </div>
      <Footer
        notice={copyNotice}
        nowMs={nowMs}
        optionsOpen={optionsOpen}
        payload={payload}
        popoverStyle={layout.popoverStyle}
        updatePending={Boolean(payload.update?.version)}
        onCheckUpdates={checkForUpdates}
        onOpenAbout={openAbout}
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
      <UpdateDialog
        checkBaseline={updateCheck.baseline}
        checkRequestedAt={updateCheck.at}
        nowMs={nowMs}
        open={updateDialogOpen}
        payload={payload}
        onCheck={requestCheck}
        onOpenChange={setUpdateDialogOpen}
      />
      <ApiKeyDialog
        keyCheck={payload.keyCheck}
        nowMs={nowMs}
        provider={keyProvider}
        returnFocus={keyFocus}
        onClose={closeKeyDialog}
      />
      <SignInDialog
        fact={payload.signIn}
        nowMs={nowMs}
        provider={signInProvider}
        returnFocus={signInFocus}
        onClose={closeSignIn}
      />
    </div>
    </PopoverActionsProvider>
    </LanguageProvider>
  );
}
