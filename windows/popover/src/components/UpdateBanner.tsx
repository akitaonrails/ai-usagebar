import MdiArrowDownCircle from "~icons/mdi/arrow-down-circle-outline";
import { HintCard } from "@/components/HintCard";
import type { UpdateInfo } from "@/lib/types";
import { useI18n } from "@/lib/i18n";
import { useBusyLabel } from "@/lib/useBusyLabel";
import { sendCommand, updateAction, updateMessage } from "../model.js";

interface UpdateBannerProps {
  repository: string;
  update: UpdateInfo;
}

/**
 * A HintCard for a waiting release, with the same action the update dialog offers: Install when
 * the host can install it here, the release page when it cannot. The ✕ snoozes this version; it
 * is hidden while a download or install is under way.
 */
export function UpdateBanner({ repository, update }: UpdateBannerProps) {
  const { t } = useI18n();
  const [clicked, startClicked] = useBusyLabel();
  const action = updateAction(update, repository);
  const busy = clicked !== null || action.busy;

  function onAction() {
    if (action.cmd === "open-url") {
      if (action.url) sendCommand("open-url", { url: action.url });
      return;
    }
    startClicked(t("Updating…"));
    sendCommand(action.cmd);
  }
  return (
    <HintCard
      actionDisabled={busy}
      buttonTitle={clicked ?? t(action.label)}
      dismissTitle={t("Remind me later")}
      icon={<MdiArrowDownCircle />}
      message={clicked ?? updateMessage(update)}
      title={t("Update available")}
      onAction={onAction}
      onDismiss={busy ? undefined : () => sendCommand("snooze-update")}
    />
  );
}
