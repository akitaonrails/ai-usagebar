import type { ReactNode } from "react";
import MdiBugOutline from "~icons/mdi/bug-outline";
import MdiGauge from "~icons/mdi/gauge";
import MdiGithub from "~icons/mdi/github";
import MdiOpenInNew from "~icons/mdi/open-in-new";
import MdiScaleBalance from "~icons/mdi/scale-balance";
import MdiTagOutline from "~icons/mdi/tag-outline";
import type { Payload } from "@/lib/types";
import { useI18n } from "@/lib/i18n";
import { sendCommand } from "../model.js";

interface AboutProps {
  payload: Payload;
}

/**
 * What AI Usage is and where the project lives. The version and the update check are not
 * repeated here: the footer shows the one and Options → Check for Updates runs the other.
 */
export function About({ payload }: AboutProps) {
  const { t } = useI18n();
  const repository = payload.repository;
  const owner = repository.replace("https://github.com/", "").split("/")[0];
  // Whole sentences, so each language can place the host's name where its grammar wants it.
  const summary =
    payload.os === "windows"
      ? t(
          "How much of each AI plan you have left — Claude, Codex, Cursor, Copilot and many more — right in the system tray, with reset times and pacing so a limit never catches you mid-task.",
        )
      : t(
          "How much of each AI plan you have left — Claude, Codex, Cursor, Copilot and many more — right in the menu bar, with reset times and pacing so a limit never catches you mid-task.",
        );

  return (
    <div className="flex flex-col gap-[var(--section-gap)]">
      <div className="card-surface flex flex-col items-center gap-[var(--section-gap)] px-[var(--card-pad)] py-[var(--section-gap)] text-center">
        <span className="grid size-[var(--app-mark)] place-items-center rounded-[var(--card-radius)] bg-[var(--accent)] text-white [&_svg]:size-[var(--app-mark-icon)]">
          <MdiGauge />
        </span>
        <span className="text-[length:var(--sz-header)] font-semibold">AI Usage</span>
        <p className="m-0 text-[length:var(--sz-support)] leading-[var(--leading-note)] text-label-2">{summary}</p>
        <p className="m-0 text-[length:var(--sz-badge)] leading-[var(--leading-note)] text-label-2">
          {t("The same readings power the ai-usagebar CLI, the terminal TUI and the Waybar, GNOME and KDE widgets.")}
          {owner ? ` ${t("Open source, by {owner} and contributors.").replace("{owner}", owner)}` : ""}
        </p>
      </div>
      {repository ? (
        <div className="card-surface">
          <LinkRow icon={<MdiGithub />} subtitle={repository.replace("https://", "")} title={t("Source Code")} url={repository} />
          <LinkRow
            icon={<MdiTagOutline />}
            subtitle={t("What changed in each version")}
            title={t("Release Notes")}
            url={`${repository}/releases`}
          />
          <LinkRow
            icon={<MdiBugOutline />}
            subtitle={t("Bugs, provider requests, ideas")}
            title={t("Report an Issue")}
            url={`${repository}/issues`}
          />
          <LinkRow
            icon={<MdiScaleBalance />}
            subtitle={t("MIT — free to use, change and share")}
            title={t("License")}
            url={`${repository}/blob/main/LICENSE`}
          />
        </div>
      ) : null}
    </div>
  );
}

interface LinkRowProps {
  icon: ReactNode;
  subtitle: string;
  title: string;
  url: string;
}

function LinkRow({ icon, subtitle, title, url }: LinkRowProps) {
  return (
    <button
      type="button"
      className="cross-link hover-row flex items-center gap-[var(--row-gap)] bg-transparent py-[var(--pad-control)] text-left"
      onClick={() => sendCommand("open-url", { url })}
    >
      <span className="grid size-[var(--row-icon-box)] shrink-0 place-items-center text-label-2 [&_svg]:size-[var(--icon-menu)]">
        {icon}
      </span>
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="truncate text-[length:var(--sz-label)] font-semibold">{title}</span>
        <span className="truncate text-[length:var(--sz-badge)] text-label-2">{subtitle}</span>
      </span>
      <MdiOpenInNew aria-hidden="true" className="size-[var(--icon-inline)] shrink-0 text-label-3" />
    </button>
  );
}
