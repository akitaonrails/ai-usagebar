import { useState, type KeyboardEvent } from "react";
import MdiCloseCircle from "~icons/mdi/close-circle";
import { useI18n } from "@/lib/i18n";
import { shortcutFromKeyEvent } from "../model.js";

interface ShortcutRecorderProps {
  error: string;
  value: string;
  onChange: (value: string) => void;
}

/**
 * Global-shortcut field: a small capsule that shows the current chord ("Ctrl+Alt+U" or "None").
 * Clicking it starts recording — the next non-modifier chord becomes the value, Escape or losing
 * focus cancels. While recording the button carries `data-recording`, which App's key guard uses
 * to keep Escape / Enter from navigating.
 */
export function ShortcutRecorder({ error, value, onChange }: ShortcutRecorderProps) {
  const { t } = useI18n();
  const [recording, setRecording] = useState(false);

  function onKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
    if (!recording) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") {
      setRecording(false);
      return;
    }
    const next = shortcutFromKeyEvent(event.nativeEvent);
    if (!next) return;
    onChange(next);
    setRecording(false);
  }

  return (
    <span className="flex shrink-0 items-center gap-[6px]">
      <button
        type="button"
        aria-invalid={error ? true : undefined}
        aria-label={recording ? t("Press keys") : value ? `${t("Global Shortcut")} ${value}` : t("Set global shortcut")}
        className="recorder"
        data-empty={value ? undefined : "true"}
        data-recording={recording ? "true" : undefined}
        title={recording ? t("Press the new shortcut, Escape to cancel") : t("Click to record a shortcut")}
        onBlur={() => setRecording(false)}
        onClick={() => setRecording(true)}
        onKeyDown={onKeyDown}
      >
        {recording ? t("Press keys…") : value || t("None")}
      </button>
      {value && !recording ? (
        <button
          type="button"
          aria-label={t("Clear shortcut")}
          className="plain-btn grid place-items-center text-label-3"
          title={t("Clear shortcut")}
          onClick={() => onChange("")}
        >
          <MdiCloseCircle className="size-[14px]" />
        </button>
      ) : null}
    </span>
  );
}
