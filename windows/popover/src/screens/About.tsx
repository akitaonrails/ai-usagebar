import type { ReactNode } from "react";
import MdiBugOutline from "~icons/mdi/bug-outline";
import MdiGauge from "~icons/mdi/gauge";
import MdiGithub from "~icons/mdi/github";
import MdiOpenInNew from "~icons/mdi/open-in-new";
import MdiScaleBalance from "~icons/mdi/scale-balance";
import MdiTagOutline from "~icons/mdi/tag-outline";
import { TruncatedText } from "@/components/TruncatedText";
import type { Payload } from "@/lib/types";
import { m } from "@/paraglide/messages.js";
import { sendCommand } from "../model.js";

interface AboutProps {
  payload: Payload;
}

/**
 * What AI Usage is and where the project lives. The version and the update check are not
 * repeated here: the footer shows the one and Options → Check for Updates runs the other.
 */
export function About({ payload }: AboutProps) {
  const repository = payload.repository;
  const owner = repository.replace("https://github.com/", "").split("/")[0];
  const summary = payload.os === "windows" ? m.about_summary_tray() : m.about_summary_menu_bar();

  return (
    <div className="flex flex-col gap-[var(--section-gap)]">
      <div className="card-surface flex flex-col items-center gap-[var(--section-gap)] px-[var(--card-pad)] py-[var(--section-gap)] text-center">
        <span className="grid size-[var(--app-mark)] place-items-center rounded-[var(--card-radius)] bg-[var(--accent)] text-white [&_svg]:size-[var(--app-mark-icon)]">
          <MdiGauge />
        </span>
        <span className="text-[length:var(--sz-header)] font-semibold">AI Usage</span>
        <p className="m-0 text-[length:var(--sz-support)] leading-[var(--leading-note)] text-label-2">{summary}</p>
        <p className="m-0 text-[length:var(--sz-badge)] leading-[var(--leading-note)] text-label-2">
          {m.same_readings_power()}
          {owner ? ` ${m.open_source_by({ owner })}` : ""}
        </p>
      </div>
      {repository ? (
        <div className="card-surface">
          <LinkRow icon={<MdiGithub />} subtitle={repository.replace("https://", "")} title={m.source_code()} url={repository} />
          <LinkRow
            icon={<MdiTagOutline />}
            subtitle={m.what_changed_each_version()}
            title={m.release_notes()}
            url={`${repository}/releases`}
          />
          <LinkRow
            icon={<MdiBugOutline />}
            subtitle={m.bugs_provider_requests_ideas()}
            title={m.report_issue()}
            url={`${repository}/issues`}
          />
          <LinkRow
            icon={<MdiScaleBalance />}
            subtitle={m.mit_license_summary()}
            title={m.license()}
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
        <TruncatedText className="text-[length:var(--sz-label)] font-semibold">{title}</TruncatedText>
        <TruncatedText className="text-[length:var(--sz-badge)] text-label-2">{subtitle}</TruncatedText>
      </span>
      <MdiOpenInNew aria-hidden="true" className="size-[var(--icon-inline)] shrink-0 text-label-3" />
    </button>
  );
}
