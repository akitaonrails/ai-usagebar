import { useCallback, useEffect, useRef, useState } from "react";
import MdiCheckCircle from "~icons/mdi/check-circle";
import MdiConsole from "~icons/mdi/console";
import MdiContentCopy from "~icons/mdi/content-copy";
import MdiLoading from "~icons/mdi/loading";
import MdiOpenInNew from "~icons/mdi/open-in-new";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogTitle } from "@/components/ui/dialog";
import type { Language, SignInFact } from "@/lib/types";
import { useI18n } from "@/lib/i18n";
import { countdownText, sendCommand, signInPhase } from "../model.js";
import { m } from "@/paraglide/messages.js";

/** The catalog row that opens the sheet, with its account and code issuer. */
export interface SignInProvider {
  /** Whose account the provider's page signs in ("ChatGPT"). */
  account: string;
  /** The provider's mark slug, as the list row draws it. */
  icon: string;
  id: string;
  /** Who hands out the code ("OpenAI"). */
  issuer: string;
  name: string;
}

// The next start's number, on the same terms as the key dialog's: it starts
// on this clock and grows by one per start, and the host echoes it back.
let nextSignInSeq = Date.now();

/** How long the sheet shows the approval before it closes itself. */
const DONE_MS = 1_500;
/** How long "Copied" stands in for "Copy". */
const COPIED_MS = 1_500;

interface SignInDialogProps {
  /** The host's last sign-in fact, from the payload's `sign_in`. */
  fact: SignInFact | null;
  nowMs: number;
  terminalAvailable?: boolean;
  onClose: () => void;
  /** The provider to sign in; null closes the sheet. */
  provider: SignInProvider | null;
  /** Where focus returns once the sheet closes, asked at that moment. */
  returnFocus?: () => HTMLElement | null;
}

/**
 * Sign a device-code provider in without a terminal. Opening the sheet asks
 * the host for a code; the host runs the provider's own login (or, for Nous,
 * this project's), shows the code it printed, and says how the flow ended.
 * The page address never reaches the webview: "Open page" asks the host to
 * open the one it holds for this start.
 */
export function SignInDialog({ provider, ...rest }: SignInDialogProps) {
  // Mounted per opening, so one opening's verdict can never close the next.
  return provider ? <OpenSignInDialog provider={provider} {...rest} /> : null;
}

function OpenSignInDialog({ fact, nowMs, onClose, provider, returnFocus, terminalAvailable = true }: SignInDialogProps & { provider: SignInProvider }) {
  const { language } = useI18n();
  const [pending, setPending] = useState<{ seq: number | null; sentAtMs: number }>({ seq: null, sentAtMs: 0 });
  const [copied, setCopied] = useState(false);
  const phase = signInPhase(fact, pending.seq, pending.sentAtMs, nowMs);
  const mine = fact && fact.seq === pending.seq ? fact : null;
  // The last code shown, kept through the expiry so the expired sheet can
  // show it dimmed, as the design does.
  const lastCode = useRef("");
  if (mine && mine.code) lastCode.current = mine.code;

  const start = useCallback(() => {
    const seq = nextSignInSeq++;
    lastCode.current = "";
    setCopied(false);
    setPending({ seq, sentAtMs: Date.now() });
    sendCommand("start-sign-in", { id: provider.id, seq });
  }, [provider.id]);

  // One start per opening; the guard keeps a re-run effect from asking twice.
  const started = useRef(false);
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    start();
  }, [start]);

  // Every way out ends the flow the sheet started: the host stops the CLI and
  // drops the fact, or drops the verdict that is already in.
  const close = useCallback(() => {
    if (pending.seq !== null) sendCommand("cancel-sign-in", { seq: pending.seq });
    onClose();
  }, [pending.seq, onClose]);

  // An approval stays on screen for a beat, then the card takes over.
  useEffect(() => {
    if (phase !== "ok") return;
    const timer = window.setTimeout(close, DONE_MS);
    return () => window.clearTimeout(timer);
  }, [phase, close]);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), COPIED_MS);
    return () => window.clearTimeout(timer);
  }, [copied]);

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
    >
      <DialogContent
        onCloseAutoFocus={(event) => {
          // Opened without a Radix trigger: hand focus back to where the list says it belongs.
          const target = returnFocus ? returnFocus() : null;
          if (!target) return;
          event.preventDefault();
          target.focus();
        }}
      >
        <SignInDialogView
          code={mine?.code || lastCode.current}
          copied={copied}
          error={mine?.error || ""}
          expiresAt={mine?.expiresAt || 0}
          language={language}
          nowMs={nowMs}
          phase={phase}
          provider={provider}
          reason={mine?.reason || ""}
          onCancel={close}
          onCopy={() => {
            if (pending.seq === null) return;
            sendCommand("copy-sign-in-code", { seq: pending.seq });
            setCopied(true);
          }}
          onOpenPage={() => {
            if (pending.seq !== null) sendCommand("open-sign-in-page", { seq: pending.seq });
          }}
          onRetry={start}
          onTerminal={terminalAvailable ? () => {
            sendCommand("sign-in-provider", { id: provider.id });
            close();
          } : undefined}
        />
      </DialogContent>
    </Dialog>
  );
}

interface SignInDialogViewProps {
  code: string;
  copied: boolean;
  error: string;
  expiresAt: number;
  language: Language;
  nowMs: number;
  phase: ReturnType<typeof signInPhase>;
  provider: SignInProvider;
  reason: string;
  onCancel: () => void;
  onCopy: () => void;
  onOpenPage: () => void;
  onRetry: () => void;
  onTerminal?: () => void;
}

/**
 * The sheet's content, without the portal, so the contract's SSR tests can
 * render the real markup. Laid out as the approved design: the provider's
 * mark beside "Sign in to …" while a code is coming or on screen, the code in
 * its box with Copy, the wait with the time left, then Cancel and Open page;
 * a check when it landed; the reason and the next step when it did not.
 */
export function SignInDialogView({
  code,
  copied,
  error,
  expiresAt,
  language,
  nowMs,
  phase,
  provider,
  reason,
  onCancel,
  onCopy,
  onOpenPage,
  onRetry,
  onTerminal,
}: SignInDialogViewProps) {
  const locale = { locale: language };
  const title = m.sign_in_dialog_title({ provider: provider.name }, locale);
  const note = "m-0 break-words text-[length:var(--sz-badge)] leading-[var(--leading-note)]";
  const cancel = (
    <button type="button" className="action-btn" onClick={onCancel}>
      {m.cancel(locale)}
    </button>
  );

  if (phase === "ok") {
    return (
      <div className="flex flex-col items-center gap-[var(--gap-item)] py-[var(--gap-stack)] text-center">
        <MdiCheckCircle className="sign-in-done" aria-hidden />
        <DialogTitle>{m.sign_in_done({ provider: provider.name }, locale)}</DialogTitle>
        <DialogDescription className="m-0 text-[length:var(--sz-badge)] text-label-2">{m.sign_in_fetching(locale)}</DialogDescription>
      </div>
    );
  }

  if (phase === "expired" || phase === "unavailable" || phase === "failed" || phase === "no-answer") {
    const message =
      phase === "expired" ? m.sign_in_code_expired(locale)
        : phase === "no-answer" ? m.sign_in_no_answer(locale)
          : reason === "device-off" ? m.sign_in_device_off({ account: provider.account }, locale)
            : reason === "cli-missing" ? m.sign_in_cli_missing({ provider: provider.name }, locale)
              : reason === "custom-path" ? m.sign_in_custom_path({ provider: provider.name }, locale)
                : reason === "denied" ? m.sign_in_denied(locale)
                  : m.sign_in_failed(locale);
    // A failure's own words, under the sheet's sentence, for the cases the
    // sentence cannot name.
    const detail = phase === "failed" && reason === "" ? error : "";
    return (
      <div className="flex flex-col gap-[var(--gap-stack)]">
        <DialogTitle>{title}</DialogTitle>
        {phase === "expired" && code ? (
          <div className="sign-in-code" data-dimmed="">
            <span className="sign-in-code-text">{code}</span>
          </div>
        ) : null}
        <DialogDescription className={`${note} text-meter-red`}>{message}</DialogDescription>
        {detail ? <p className={`${note} text-label-2`}>{detail}</p> : null}
        <DialogFooter>
          {cancel}
          {phase === "expired" ? (
            <button type="button" className="action-btn" data-variant="primary" onClick={onRetry}>
              {m.sign_in_new_code(locale)}
            </button>
          ) : phase === "unavailable" && onTerminal ? (
            <button type="button" className="action-btn gap-1.5 [&_svg]:size-[var(--icon-note)]" onClick={onTerminal}>
              <MdiConsole aria-hidden />
              {m.sign_in_use_terminal(locale)}
            </button>
          ) : phase === "failed" ? (
            <button type="button" className="action-btn" data-variant="primary" onClick={onRetry}>
              {m.sign_in_try_again(locale)}
            </button>
          ) : null}
        </DialogFooter>
      </div>
    );
  }

  const header = (
    <div className="flex items-center gap-[var(--gap-item)]">
      <span className="key-dialog-mark">
        <ProviderIcon className="text-label-2" size="var(--icon-row)" slug={provider.icon} title={provider.name} />
      </span>
      <div className="flex min-w-0 flex-col">
        <DialogTitle>{title}</DialogTitle>
        {phase === "waiting" ? (
          <DialogDescription className="m-0 text-[length:var(--sz-badge)] text-label-2">
            {m.sign_in_with_account({ account: provider.account }, locale)}
          </DialogDescription>
        ) : null}
      </div>
    </div>
  );

  if (phase !== "waiting") {
    // Starting (and the instant before the first fact): the code is on its way.
    return (
      <div className="flex flex-col gap-[var(--gap-stack)]">
        {header}
        <div className="sign-in-code" role="status">
          <MdiLoading className="size-[var(--icon-note)] shrink-0 animate-spin text-[var(--accent)]" aria-hidden />
          <span className="text-[length:var(--sz-support)] text-label-2">{m.sign_in_asking({ issuer: provider.issuer }, locale)}</span>
        </div>
        <DialogFooter>{cancel}</DialogFooter>
      </div>
    );
  }

  const left = expiresAt > 0 ? countdownText(expiresAt, nowMs) : "";
  return (
    <div className="flex flex-col gap-[var(--gap-stack)]">
      {header}
      <p className="m-0 text-[length:var(--sz-support)]">{m.sign_in_enter_code(locale)}</p>
      <div className="sign-in-code">
        <span className="sign-in-code-text select-all">{code}</span>
        <button type="button" className="action-btn gap-1.5 [&_svg]:size-[var(--icon-note)]" disabled={!code} onClick={onCopy}>
          <MdiContentCopy aria-hidden />
          {copied ? m.sign_in_copied(locale) : m.sign_in_copy(locale)}
        </button>
      </div>
      <div className="flex items-start gap-[var(--gap-item)] text-label-2" role="status">
        <MdiLoading className="mt-[var(--space-px)] size-[var(--icon-note)] shrink-0 animate-spin text-[var(--accent)]" aria-hidden />
        <span className={`${note} flex-1`}>{m.sign_in_waiting(locale)}</span>
        {left ? (
          <span className="shrink-0 text-[length:var(--sz-badge)] tabular-nums" aria-label={m.sign_in_expires_in({ time: left }, locale)}>
            {left}
          </span>
        ) : null}
      </div>
      <DialogFooter>
        {cancel}
        <button type="button" className="action-btn gap-1.5 [&_svg]:size-[var(--icon-note)]" data-variant="primary" onClick={onOpenPage}>
          {m.sign_in_open_page(locale)}
          <MdiOpenInNew aria-hidden />
        </button>
      </DialogFooter>
    </div>
  );
}
