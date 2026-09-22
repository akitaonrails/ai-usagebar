import MdiOpenInNew from "~icons/mdi/open-in-new";
import type { Payload } from "@/lib/types";
import { sendCommand, updateStatusLabel } from "../model.js";

interface AboutProps {
  nowMs: number;
  payload: Payload;
}

/** Version, release check, and the GitHub page this build was compiled from. */
export function About({ nowMs, payload }: AboutProps) {
  const status = updateStatusLabel(payload, nowMs);
  const action = updateAction(payload);
  const version = payload.version ? `Version ${payload.version}` : "Development build";

  return (
    <div className="flex flex-col gap-[var(--section-gap)]">
      <div className="card-surface">
        <div className="flex flex-col gap-1 px-[var(--card-pad)] py-[var(--pad-control)]">
          <span className="text-[length:var(--sz-header)] font-semibold">AI Usage</span>
          <span className="text-[length:var(--sz-support)] text-label-2">{version}</span>
          <span className="text-[length:var(--sz-badge)] leading-[1.35] text-label-2">{status}</span>
        </div>
        <div className="flex items-center px-[var(--card-pad)] py-[var(--pad-control)]">
          <span className="text-[length:var(--sz-label)] font-semibold">Check for Updates</span>
          <span className="min-w-2 flex-1" />
          <button
            type="button"
            className="h-[var(--control-h)] shrink-0 rounded-[var(--radius-sm)] bg-[var(--control-fill)] px-2.5 text-[length:var(--sz-support)] hover:bg-[var(--control-fill-hover)] disabled:opacity-60"
            disabled={action.disabled}
            onClick={action.run}
          >
            {action.label}
          </button>
        </div>
        {payload.repository ? (
          <div className="flex items-center px-[var(--card-pad)] py-[var(--pad-control)]">
            <span className="min-w-0 truncate text-[length:var(--sz-label)] font-semibold leading-none">
              Source on GitHub
            </span>
            <span className="min-w-2 flex-1" />
            <button
              type="button"
              className="inline-flex h-[var(--control-h)] shrink-0 items-center gap-1 border-0 bg-transparent p-0 text-[length:var(--sz-support)] leading-none text-label-2"
              onClick={() => sendCommand("open-url", { url: payload.repository })}
            >
              Open
              <MdiOpenInNew aria-hidden="true" className="size-[13px] shrink-0" />
            </button>
          </div>
        ) : null}
      </div>
    </div>
  );
}

interface UpdateAction {
  disabled: boolean;
  label: string;
  run: () => void;
}

function updateAction(payload: Payload): UpdateAction {
  const update = payload.update;
  if (update?.state === "checking") {
    return { disabled: true, label: "Checking…", run: () => {} };
  }
  if (update?.state === "downloading" || update?.state === "installing") {
    return { disabled: true, label: "Updating…", run: () => {} };
  }
  // Windows can swap the running exe. macOS has no installer yet: open the release.
  if (update?.state === "available" && payload.os !== "macos") {
    return { disabled: false, label: "Install", run: () => sendCommand("install-update") };
  }
  if (update?.state === "available" && update.url) {
    return {
      disabled: false,
      label: "View release",
      run: () => sendCommand("open-url", { url: update.url }),
    };
  }
  if (update?.state === "failed") {
    return { disabled: false, label: "Try again", run: () => sendCommand("check-update") };
  }
  return { disabled: false, label: "Check now", run: () => sendCommand("check-update") };
}
