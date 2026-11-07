import { useEffect, useLayoutEffect, useRef, type ComponentType, type MouseEvent, type ReactNode } from "react";
import MdiAlert from "~icons/mdi/alert";
import MdiChevronRight from "~icons/mdi/chevron-right";
import MdiConsole from "~icons/mdi/console";
import MdiHelpCircleOutline from "~icons/mdi/help-circle-outline";
import MdiKeyPlus from "~icons/mdi/key-plus";
import MdiLogin from "~icons/mdi/login";
import MdiOpenInApp from "~icons/mdi/open-in-app";
import MdiPlus from "~icons/mdi/plus";
import { DragHandle } from "@/components/DragHandle";
import { Hint } from "@/components/Hint";
import { IconButton } from "@/components/IconButton";
import { ProviderIcon } from "@/components/ProviderIcon";
import { TruncatedText } from "@/components/TruncatedText";
import { SortableItem, VerticalDnd } from "@/components/dnd";
import { Switch } from "@/components/ui/switch";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { Card, Layout, ProviderInfo, ProviderListRow } from "@/lib/types";
import { handFocusOnRemoval, usePopoverActions } from "@/lib/popoverActions";
import { m } from "@/paraglide/messages.js";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { addAccountDecision, metricsCountText, multiAccountSubtitle, providerListRows, reorderProviderRows, sendCommand } from "../model.js";

interface ProviderListProps {
  cards: Card[];
  /** Why the last enable/disable failed, or "". */
  error: string;
  layout: Layout;
  onOpen: (id: string) => void;
  onReorder: (ids: string[]) => void;
  onToggle: (id: string, on: boolean) => void;
  providers: ProviderInfo[];
  /** The header's right side beside the title (Settings passes reset and Detect). */
  headerActions?: ReactNode;
  /**
   * Draw the "Providers" title and the visible how-to line. Settings has a tab
   * that supplies the heading, so it stays compact (icon only); Customize is
   * this list alone and keeps both, which is what it looked like before the
   * header was unified.
   */
  heading?: boolean;
}

const ADD_ACCOUNT_LABELS: Record<string, () => string> = { code: m.add_claude_code_account, desktop: m.add_claude_desktop_account, codex: m.add_codex_account };

/**
 * The one provider list (Customize and Settings → Providers share it): enabled
 * providers come first, in the dashboard's card order — dragging a row moves
 * the provider's whole cards — and the rest keep the catalog's order below.
 * Every enabled row is uniform: grip, mark, name, subtitle, a discreet "+"
 * that adds an account, the switch that writes the provider's `enabled` to
 * config.toml, and a chevron to the provider's detail. A provider's accounts
 * live in its subtitle; the accounts themselves are chosen there, in the
 * detail, not in this list.
 */
export function ProviderList({ cards, error, headerActions, heading = false, layout, onOpen, onReorder, onToggle, providers }: ProviderListProps) {
  // The host re-reads credentials only when asked, so ask whenever the list opens.
  useEffect(() => {
    sendCommand("providers");
  }, []);
  const actions = usePopoverActions();
  // The credential's fix, from the "+" column: the key dialog for a key-only
  // provider, the sign-in sheet for a device-code one, the host otherwise.
  const onAddKey = (row: ProviderListRow, trigger: HTMLElement) => {
    if (row.action === "settings" && row.keyLabel) actions.addKey(row.id, trigger);
    else if (row.action === "device") actions.signIn(row.id, trigger);
    else sendCommand("sign-in-provider", { id: row.id });
  };
  const { enabled, disabled } = providerListRows(providers, cards, layout);
  return (
    <div className="flex flex-col gap-[var(--header-card-gap)]">
      <div className="flex items-center justify-between gap-2">
        <div className="section-title flex items-center gap-[var(--gap-inline)]">
          {heading ? <span>{m.providers()}</span> : null}
          <Hint content={m.all_providers_hint()}>
            <span tabIndex={0} role="img" aria-label={m.all_providers_hint()}>
              <MdiHelpCircleOutline className="size-[var(--icon-help)] shrink-0 text-label-2" />
            </span>
          </Hint>
        </div>
        {headerActions ? (
          <div className="flex items-center gap-2">
            {headerActions}
          </div>
        ) : null}
      </div>
      {enabled.length + disabled.length > 0 ? (
        <div className="card-surface">
          <VerticalDnd
            items={enabled.map((row) => row.id)}
            onReorder={(ids) => onReorder(reorderProviderRows(layout.cardOrder, enabled, ids))}
            overlay={(id) => {
              const row = enabled.find((item) => item.id === id);
              if (!row) return null;
              return (
                <div className="lifted-surface">
                  <ProviderRow onAddKey={onAddKey} onOpen={onOpen} onToggle={onToggle} row={row} />
                </div>
              );
            }}
          >
            {enabled.map((row) => (
              <SortableItem key={row.id} id={row.id}>
                {({ attributes, listeners }) => (
                  <ProviderRow handle={{ attributes, listeners }} onAddKey={onAddKey} onOpen={onOpen} onToggle={onToggle} row={row} />
                )}
              </SortableItem>
            ))}
            {disabled.map((row) => (
              <ProviderRow key={row.id} off onAddKey={onAddKey} onOpen={onOpen} onToggle={onToggle} row={row} />
            ))}
          </VerticalDnd>
        </div>
      ) : null}
      {error ? (
        <div className="px-[var(--card-pad)] text-[length:var(--sz-badge)] text-meter-red">
          {error}
        </div>
      ) : heading ? (
        // Customize has no tab to explain the list; keep the instruction
        // visible instead of hiding it behind the help icon.
        <div className="px-[var(--card-pad)] text-[length:var(--sz-badge)] text-label-2">
          {m.all_providers_hint()}
        </div>
      ) : null}
    </div>
  );
}

interface ProviderRowProps {
  handle?: Parameters<typeof DragHandle>[0];
  /** Drawn in the disabled block: faded, no drag handle, an inert chevron. */
  off?: boolean;
  /** Runs the credential's action: the key dialog, the sign-in sheet, or the host's sign-in. */
  onAddKey: (row: ProviderListRow, trigger: HTMLElement) => void;
  onOpen: (id: string) => void;
  onToggle: (id: string, on: boolean) => void;
  row: ProviderListRow;
}

/**
 * One provider row, uniform across the enabled block: grip, mark, name +
 * subtitle, the discreet "+" (a direct action when the catalog offers one
 * account kind, a menu when it offers several, absent otherwise — its space
 * is kept so the columns line up), the switch, the chevron. The subtitle
 * reads "N metrics" for one card and "Default, work · 6 metrics" for
 * several; the catalog's status stands in while the provider has no card.
 * The row that needs its credential breaks the uniform: no subtitle — the
 * hint rides an alert icon next to the name, and the action takes the "+"
 * column as an icon, so the name never cuts.
 */
function ProviderRow({ row, handle, off = false, onAddKey, onOpen, onToggle }: ProviderRowProps) {
  const { language } = useI18n();
  const first = !off && row.cards.length > 0 ? row.cards[0] : null;
  const needs = row.state === "needs";
  // A row that needs its credential has no subtitle at all: the hint rides
  // the alert icon beside the name, so the error card's "0 metrics" can
  // neither replace it nor make it flash and vanish.
  const statusText = needs ? ""
    : row.state === "off" ? m.provider_off()
      : row.state === "off-ready" ? m.provider_off_ready()
        : row.state === "ready" ? m.provider_ready()
          : "";
  const metrics = !off && row.cards.length >= 1 && !needs;
  const subtitle = metrics && row.cards.length === 1
    ? metricsCountText(row.cards[0].metrics, language)
    : metrics
      ? multiAccountSubtitle(row.cards, row.metrics, row.name, language)
      : statusText;
  // The credential's action wears the "+" column as an icon, and while it is
  // there the add-account "+" stands down for the row. A key-only provider's
  // "Add key" opens the key dialog, a device-code provider's "Sign in" the
  // sign-in sheet; the others still go through the host.
  const action = needs && row.action
    ? row.action === "settings" ? { icon: MdiKeyPlus, label: m.provider_add_key() }
      : row.action === "device" ? { icon: MdiLogin, label: m.provider_sign_in() }
        : row.action === "terminal" ? { icon: MdiConsole, label: m.provider_sign_in() }
          : { icon: MdiOpenInApp, label: m.provider_open_app() }
    : null;
  const add = addAccountDecision(row);
  // The host's hint names the TUI or a terminal command; a provider whose fix
  // is here (the key dialog, the sign-in sheet) points at its button instead.
  const hint = row.action === "settings" && row.keyLabel ? m.needs_api_key_hint()
    : row.action === "device" && row.signInAccount ? m.sign_in_to_track_usage({ account: row.signInAccount })
      : row.signIn;
  const identity = (
    <>
      <ProviderIcon className="text-label-2" size="var(--row-icon-box)" slug={row.icon} title={row.name} />
      <span className="flex min-w-0 flex-col">
        <TruncatedText className="text-[length:var(--sz-header)] font-semibold">{row.name}</TruncatedText>
        {subtitle ? (
          <span className="text-[length:var(--sz-badge)] break-words text-label-2">{subtitle}</span>
        ) : null}
      </span>
      {needs && hint ? (
        <Hint content={hint}>
          <span tabIndex={0} role="img" aria-label={hint}>
            <MdiAlert className="size-[var(--icon-mark)] shrink-0 text-notice" />
          </span>
        </Hint>
      ) : null}
    </>
  );
  return (
    <div data-provider-id={row.id} className={cn("flex flex-col", off && "opacity-55")}>
      <div className="flex items-center gap-[var(--row-gap)] px-[var(--card-pad)] py-[var(--pad-control)]">
        {handle ? (
          <DragHandle attributes={handle?.attributes} listeners={handle?.listeners} />
        ) : (
          // Rows without a drag target (off block, overlay) get the same 16×22
          // footprint as the grip, so the name line stays aligned — no handle.
          <span className="grip" aria-hidden />
        )}
        {first && !needs ? (
          // A row that needs its credential carries the focusable alert icon,
          // which cannot sit inside a button; its chevron still opens the card.
          <button type="button" className="plain-btn hover-fill flex min-w-0 flex-1 items-center gap-[var(--row-gap)]" onClick={() => onOpen(first.id)}>
            {identity}
          </button>
        ) : (
          <div className="flex min-w-0 flex-1 items-center gap-[var(--row-gap)]">{identity}</div>
        )}
        {action ? (
          <CredentialAction
            icon={action.icon}
            label={action.label}
            rowId={row.id}
            onClick={(event) => onAddKey(row, event.currentTarget)}
          />
        ) : add ? (
          add.kind === "direct" ? (
            <IconButton label={m.add_account()} onClick={() => sendCommand("add-account", { kind: add.value })}>
              <MdiPlus />
            </IconButton>
          ) : (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <IconButton label={m.add_account()}>
                  <MdiPlus />
                </IconButton>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" sideOffset={2}>
                {add.value.map((kind) => (
                  <DropdownMenuItem key={kind} onSelect={() => sendCommand("add-account", { kind })}>
                    <MdiPlus />
                    <span className="flex-1">{(ADD_ACCOUNT_LABELS[kind] || m.add_account)()}</span>
                  </DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
          )
        ) : (
          // No account kinds (or the provider is off): keep the column so the
          // switch and chevron stay aligned across rows.
          <span aria-hidden className="size-[var(--icon-btn)]" />
        )}
        {row.switchable ? (
          <Switch
            checked={row.enabled}
            aria-label={`${m.provider_enable()} ${row.name}`}
            onCheckedChange={(on) => onToggle(row.id, on === true)}
          />
        ) : (
          // A custom provider toggles in config.toml by hand: keep the slot so
          // the chevron stays in its column.
          <span aria-hidden className="w-[var(--switch-w)]" />
        )}
        {first ? (
          <IconButton label={`${m.open()} ${row.cards.length > 1 ? row.name : first.title}`} onClick={() => onOpen(first.id)}>
            <MdiChevronRight />
          </IconButton>
        ) : (
          // Nothing to open (the provider is off, or has no card yet): the
          // chevron stays, inert and dimmed, so every switch keeps its column.
          <span aria-hidden className="icon-btn" data-inert>
            <MdiChevronRight />
          </span>
        )}
      </div>
    </div>
  );
}

interface CredentialActionProps {
  icon: ComponentType<{ className?: string }>;
  label: string;
  onClick: (event: MouseEvent<HTMLButtonElement>) => void;
  rowId: string;
}

/**
 * The credential's action in the "+" column. It disappears once the provider
 * has its credential — often while it still holds focus, after the key
 * dialog handed focus back to it and the next catalog arrived. Removed while
 * focused, it passes focus to its row's switch instead of dropping it on the
 * body, where Enter would act on the whole screen.
 */
function CredentialAction({ icon: Icon, label, onClick, rowId }: CredentialActionProps) {
  const button = useRef<HTMLButtonElement | null>(null);
  useLayoutEffect(() => {
    const node = button.current;
    // A layout cleanup on removal runs while the node is still in the document.
    return () => handFocusOnRemoval(node, rowId);
  }, [rowId]);
  return (
    <IconButton ref={button} label={label} data-key-row={rowId} onClick={onClick}>
      <Icon />
    </IconButton>
  );
}
