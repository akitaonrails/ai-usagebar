import MdiArrowDownCircle from "~icons/mdi/arrow-down-circle-outline";
import MdiClose from "~icons/mdi/close";
import type { UpdateInfo } from "@/lib/types";
import { useBusyLabel } from "@/lib/useBusyLabel";
import { sendCommand } from "../model.js";

interface UpdateBannerProps {
  update: UpdateInfo;
}

const ACTION_LABEL: Record<UpdateInfo["state"], string> = {
  available: "Install Update",
  checking: "Checking…",
  downloading: "Downloading…",
  failed: "Try Again",
  installing: "Installing…",
};

/**
 * Same anatomy as HintCard: icon, title + message + small action button, ✕ in the corner. The
 * ✕ snoozes the update; it is hidden while a download or install is under way.
 */
export function UpdateBanner({ update }: UpdateBannerProps) {
  const [clicked, startClicked] = useBusyLabel();

  const busy = clicked !== null || update.state === "downloading" || update.state === "installing";
  const version = update.version.replace(/^v/i, "");
  const message =
    clicked ?? (update.state === "failed" ? `Couldn't update: ${update.error}` : `AI Usage v${version} is ready to install.`);
  const actionLabel = clicked ?? ACTION_LABEL[update.state];

  function onInstall() {
    startClicked("Updating…");
    sendCommand("install-update");
  }
  return (
    <div className="card-surface flex items-start gap-[10px] px-[14px] py-3">
      <span className="grid size-5 shrink-0 place-items-center text-label-2 [&_svg]:size-4">
        <MdiArrowDownCircle />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="text-[length:var(--sz-label)] font-semibold">Update available</span>
        <span className="text-[length:var(--sz-support)] leading-[1.35] text-label-2" title={update.url || undefined}>
          {message}
        </span>
        <button
          type="button"
          className="mt-1 h-6 w-fit rounded-[6px] bg-[var(--control-fill)] px-2.5 text-[length:var(--sz-support)] hover:bg-[var(--control-fill-hover)] disabled:opacity-60"
          disabled={busy}
          onClick={onInstall}
        >
          {actionLabel}
        </button>
      </div>
      {busy ? null : (
        <button
          type="button"
          aria-label="Dismiss"
          className="plain-btn grid size-4 shrink-0 place-items-center text-label-2"
          title="Remind me later"
          onClick={() => sendCommand("snooze-update")}
        >
          <MdiClose className="size-3" />
        </button>
      )}
    </div>
  );
}
