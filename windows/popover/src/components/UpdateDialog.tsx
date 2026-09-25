import type { ReactNode } from "react";
import MdiAlertCircle from "~icons/mdi/alert-circle-outline";
import MdiArrowDownCircle from "~icons/mdi/arrow-down-circle-outline";
import MdiCheckCircle from "~icons/mdi/check-circle-outline";
import MdiLoading from "~icons/mdi/loading";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogTitle } from "@/components/ui/dialog";
import type { Payload, UpdateAction } from "@/lib/types";
import { cn } from "@/lib/utils";
import { m } from "@/paraglide/messages.js";
import { useI18n } from "@/lib/i18n";
import { sendCommand, updateAction, updateMessage } from "../model.js";

/** A check with no answer by now is reported, not left spinning. */
const NO_ANSWER_MS = 30_000;

interface UpdateDialogProps {
  /** The host's `updateCheckedAt` when the user asked; any other stamp is the answer. */
  checkBaseline: number;
  /** `Date.now()` when the user last asked for a check, for the no-answer timeout. */
  checkRequestedAt: number;
  nowMs: number;
  open: boolean;
  payload: Payload;
  onCheck: () => void;
  onOpenChange: (open: boolean) => void;
}

interface View {
  icon: ReactNode;
  message: string;
  primary: UpdateAction | null;
  secondary: string;
  title: string;
  tone: "accent" | "green" | "muted" | "red";
}

/**
 * Options → Check for Updates: the check runs in place, over whatever screen is open. While the
 * host has not answered this check, the last known state is stale and the dialog says "Checking";
 * then it shows the answer with the one action that applies (Install, View Release, Try Again).
 */
export function UpdateDialog({
  checkBaseline,
  checkRequestedAt,
  nowMs,
  open,
  payload,
  onCheck,
  onOpenChange,
}: UpdateDialogProps) {
  const { language } = useI18n();
  const view = viewFor(payload, checkBaseline, checkRequestedAt, nowMs, language);

  function run(action: UpdateAction) {
    if (action.cmd === "open-url") {
      if (action.url) sendCommand("open-url", { url: action.url });
      onOpenChange(false);
    } else if (action.cmd === "check-update" || (action.cmd === "install-update" && !payload.update?.version)) {
      // A failed check has nothing to install: the host checks again, and so do we.
      onCheck();
    } else if (action.cmd) {
      sendCommand(action.cmd);
    }
  }

  /** Enter on the card presses the default button: the action if there is one, else the dismiss. */
  function runDefault() {
    if (view.primary) {
      if (!view.primary.busy) run(view.primary);
    } else if (view.secondary) {
      onOpenChange(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        onOpenAutoFocus={(event) => {
          // Focus the card, not a button: no ring on open, and nothing focused behind the scrim.
          event.preventDefault();
          (event.currentTarget as HTMLElement).focus();
        }}
        onKeyDown={(event) => {
          // A focused button already answers Enter itself.
          if (event.key !== "Enter" || event.target !== event.currentTarget) return;
          event.preventDefault();
          runDefault();
        }}
      >
        <div className="flex flex-col gap-[var(--gap-stack)]">
          <div className="flex items-center gap-[var(--gap-item)]">
            <span
              className={cn(
                "grid shrink-0 place-items-center [&_svg]:size-[var(--icon-row)]",
                view.tone === "accent" && "text-[var(--accent)]",
                view.tone === "green" && "text-[var(--green)]",
                view.tone === "red" && "text-[var(--red)]",
                view.tone === "muted" && "text-label-2",
              )}
            >
              {view.icon}
            </span>
            <DialogTitle>{view.title}</DialogTitle>
          </div>
          <DialogDescription>{view.message}</DialogDescription>
        </div>
        <DialogFooter>
          {view.secondary ? (
            <button type="button" className="action-btn" onClick={() => onOpenChange(false)}>
              {view.secondary}
            </button>
          ) : null}
          {view.primary ? (
            <button
              type="button"
              className="action-btn"
              data-variant="primary"
              disabled={view.primary.busy}
              onClick={() => view.primary && run(view.primary)}
            >
              {view.primary.label}
            </button>
          ) : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function viewFor(payload: Payload, checkBaseline: number, checkRequestedAt: number, nowMs: number, language: "en" | "pt-BR"): View {
  const update = payload.update;
  const state = update?.state;
  const action = updateAction(update, payload.repository, language);
  const spinner = <MdiLoading className="animate-spin" />;

  if (state === "downloading" || state === "installing") {
    return { icon: spinner, message: updateMessage(update, language), primary: action, secondary: "", title: m.updating({}, { locale: language }), tone: "muted" };
  }
  // Compare the host's stamps with each other, never with this WebView's clock.
  const answered = payload.updateCheckedAt !== checkBaseline && state !== "checking";
  if (!answered) {
    if (nowMs - checkRequestedAt < NO_ANSWER_MS) {
      return {
        icon: spinner,
        message: m.looking_for_newer_release({}, { locale: language }),
        primary: null,
        secondary: m.cancel({}, { locale: language }),
        title: m.checking_for_updates({}, { locale: language }),
        tone: "muted",
      };
    }
    return {
      icon: <MdiAlertCircle />,
      message: m.update_check_no_answer({}, { locale: language }),
      primary: updateAction(null, payload.repository, language),
      secondary: m.close({}, { locale: language }),
      title: m.no_answer({}, { locale: language }),
      tone: "red",
    };
  }
  if (!update) {
    const version = payload.version ? `AI Usage ${payload.version}` : m.this_build({}, { locale: language });
    return {
      icon: <MdiCheckCircle />,
      message: m.latest_version({ version }, { locale: language }),
      primary: null,
      secondary: m.ok({}, { locale: language }),
      title: m.youre_up_to_date({}, { locale: language }),
      tone: "green",
    };
  }
  if (state === "failed") {
    return {
      icon: <MdiAlertCircle />,
      message: updateMessage(update, language),
      primary: action,
      secondary: m.close({}, { locale: language }),
      title: update.version ? m.couldnt_update_title({}, { locale: language }) : m.couldnt_check_for_updates_title({}, { locale: language }),
      tone: "red",
    };
  }
  return {
    icon: <MdiArrowDownCircle />,
    message: updateMessage(update, language),
    primary: action,
    secondary: m.later({}, { locale: language }),
    title: m.update_available_title({}, { locale: language }),
    tone: "accent",
  };
}
