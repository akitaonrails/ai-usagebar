import { useEffect, useState } from "react";
import MdiEye from "~icons/mdi/eye";
import MdiEyeOff from "~icons/mdi/eye-off";
import MdiLoading from "~icons/mdi/loading";
import MdiLockOutline from "~icons/mdi/lock-outline";
import { IconButton } from "@/components/IconButton";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogTitle } from "@/components/ui/dialog";
import type { KeyCheck, Language } from "@/lib/types";
import { useI18n } from "@/lib/i18n";
import { keyCheckError, keyDialogPhase, sendCommand } from "../model.js";
import { m } from "@/paraglide/messages.js";

/** The catalog row that opens the dialog, with its credential's label and note. */
export interface KeyDialogProvider {
  /** The provider's mark slug, as the list row draws it. */
  icon: string;
  id: string;
  keyLabel: string;
  keyNote: string;
  name: string;
}

// The next send's number. It starts on this clock and grows by one per send,
// so a verdict is matched to its send by number alone: the host echoes back
// the `seq` it was asked with, and never has to agree with this clock.
let nextKeySeq = Date.now();

interface ApiKeyDialogProps {
  /** The provider whose key is being set; null closes the dialog. */
  provider: KeyDialogProvider | null;
  /** The host's last verdict, from the payload's `key_check`. */
  keyCheck: KeyCheck | null;
  nowMs: number;
  onClose: () => void;
  /** Where focus returns once the dialog closes, asked at that moment. */
  returnFocus?: () => HTMLElement | null;
}

/**
 * Set an API key and let the provider judge it. Save writes the key through
 * the host's settings writer and sends it for a check of this provider alone;
 * the host keeps the key only if the provider accepts it, and answers with the
 * verdict under the send's `seq`. The draft never stays in state after a send:
 * the host owns the value, and this dialog only ever names it on the way out.
 */
export function ApiKeyDialog({ provider, ...rest }: ApiKeyDialogProps) {
  // Mounted per opening, so a verdict from an earlier opening (the last send's
  // `ok`) can never close the next one.
  return provider ? <OpenApiKeyDialog provider={provider} {...rest} /> : null;
}

function OpenApiKeyDialog({ provider, keyCheck, nowMs, onClose, returnFocus }: ApiKeyDialogProps & { provider: KeyDialogProvider }) {
  const { language } = useI18n();
  const [key, setKey] = useState("");
  const [pending, setPending] = useState<{ seq: number | null; sentAtMs: number }>({ seq: null, sentAtMs: 0 });
  const [revealed, setRevealed] = useState(false);
  const phase = keyDialogPhase(keyCheck, pending.seq, pending.sentAtMs, nowMs);

  // An accepted key closes the dialog; the verdict that says so is the host's.
  useEffect(() => {
    if (phase === "ok") onClose();
  }, [phase, onClose]);

  function save() {
    if (key === "" || phase === "checking") return;
    const seq = nextKeySeq++;
    setPending({ seq, sentAtMs: Date.now() });
    setKey("");
    sendCommand("set-api-key", { id: provider.id, key, seq });
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent
        onCloseAutoFocus={(event) => {
          // The dialog is opened without a Radix trigger, so Radix would drop
          // focus on the body; hand it back to where the list says it belongs.
          const target = returnFocus ? returnFocus() : null;
          if (!target) return;
          event.preventDefault();
          target.focus();
        }}
      >
        <ApiKeyDialogView
          checking={phase === "checking"}
          error={phase === "failed" && keyCheck ? keyCheck.error : ""}
          keyDraft={key}
          language={language}
          noAnswer={phase === "no-answer"}
          provider={provider}
          revealed={revealed}
          onKeyChange={setKey}
          onRevealToggle={() => setRevealed((current) => !current)}
          onCancel={onClose}
          onSave={save}
        />
      </DialogContent>
    </Dialog>
  );
}

interface ApiKeyDialogViewProps {
  /** The spinner phase: the field and Save wait for the host's verdict. */
  checking: boolean;
  /** The refused key's error, already cut by the host ("" outside `failed`). */
  error: string;
  keyDraft: string;
  language: Language;
  /** The send has not answered within the limit. */
  noAnswer: boolean;
  provider: KeyDialogProvider;
  revealed: boolean;
  onKeyChange: (value: string) => void;
  onRevealToggle: () => void;
  onCancel: () => void;
  onSave: () => void;
}

// The field's stand-in for the key while the host checks it: the draft
// itself is gone from state by then.
const MASKED = "\u2022".repeat(16);

/**
 * The dialog's content, without the portal, so the contract's SSR tests can
 * render the real markup — Radix keeps portaled content out of static
 * markup, the same way it does the menus. Laid out as the approved design:
 * the provider's mark beside the title and the credential's note, the field
 * with its reveal toggle inside, the outcome right under the field, the
 * storage line, then Cancel and Save.
 */
export function ApiKeyDialogView({
  checking,
  error,
  keyDraft,
  language,
  noAnswer,
  provider,
  revealed,
  onKeyChange,
  onRevealToggle,
  onCancel,
  onSave,
}: ApiKeyDialogViewProps) {
  const locale = { locale: language };
  const saveDisabled = checking || keyDraft === "";
  // The host's note says which key this is ("admin key — monthly spend").
  const note = provider.keyNote ? provider.keyNote.charAt(0).toUpperCase() + provider.keyNote.slice(1) : "";
  return (
    <form
      className="flex flex-col gap-[var(--gap-stack)]"
      onSubmit={(event) => {
        // Enter in the field saves, like the Save button.
        event.preventDefault();
        onSave();
      }}
    >
      <div className="flex items-center gap-[var(--gap-item)]">
        <span className="key-dialog-mark">
          <ProviderIcon className="text-label-2" size="var(--icon-row)" slug={provider.icon} title={provider.name} />
        </span>
        <div className="flex min-w-0 flex-col">
          <DialogTitle>{m.api_key_dialog_title({ provider: provider.name }, locale)}</DialogTitle>
          {note ? (
            <DialogDescription className="m-0 text-[length:var(--sz-badge)] text-label-2">{note}</DialogDescription>
          ) : null}
        </div>
      </div>
      <div className="key-field" data-invalid={error ? "" : undefined}>
        <input
          type={revealed ? "text" : "password"}
          value={keyDraft}
          disabled={checking}
          autoComplete="off"
          spellCheck={false}
          autoCapitalize="off"
          placeholder={checking ? MASKED : provider.keyLabel}
          aria-label={provider.keyLabel}
          aria-invalid={error ? true : undefined}
          onChange={(event) => onKeyChange(event.target.value)}
        />
        <IconButton
          disabled={checking}
          label={revealed ? m.hide_key(locale) : m.show_key(locale)}
          onClick={onRevealToggle}
        >
          {revealed ? <MdiEyeOff /> : <MdiEye />}
        </IconButton>
      </div>
      {checking ? (
        <p className="m-0 text-[length:var(--sz-badge)] leading-[var(--leading-note)] text-label-2">
          {m.api_key_checking({ provider: provider.name }, locale)}
        </p>
      ) : null}
      {error ? (
        <p className="m-0 break-words text-[length:var(--sz-badge)] leading-[var(--leading-note)] text-meter-red">
          {keyCheckError(error, provider.name, language)}
        </p>
      ) : null}
      {noAnswer ? (
        <p className="m-0 break-words text-[length:var(--sz-badge)] leading-[var(--leading-note)] text-notice">
          {m.api_key_no_answer({ provider: provider.name }, locale)}
        </p>
      ) : null}
      <div className="flex items-center gap-[var(--gap-item)] text-label-2">
        <MdiLockOutline className="size-[var(--icon-note)] shrink-0" />
        <span className="text-[length:var(--sz-badge)]">{m.api_key_stored_note(locale)}</span>
      </div>
      <DialogFooter>
        <button type="button" className="action-btn" onClick={onCancel}>
          {m.cancel(locale)}
        </button>
        <button type="submit" className="action-btn gap-1.5 [&_svg]:size-[var(--icon-note)]" data-variant="primary" disabled={saveDisabled}>
          {checking ? <MdiLoading className="animate-spin" /> : null}
          {checking ? m.api_key_checking_short(locale) : m.api_key_save(locale)}
        </button>
      </DialogFooter>
    </form>
  );
}
