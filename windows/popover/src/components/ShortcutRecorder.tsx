import { useState, type KeyboardEvent } from "react";
import MdiCloseCircle from "~icons/mdi/close-circle";
import { m } from "@/paraglide/messages.js";
import { shortcutFromKeyEvent } from "../model.js";
import { Hint } from "@/components/Hint";

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
    <span className="flex shrink-0 items-center gap-[var(--gap-item)]">
      <Hint align="end" content={recording ? m.shortcut_recording_hint() : m.click_to_record_a_shortcut()}>
        <button
          type="button"
          aria-invalid={error ? true : undefined}
          aria-label={recording ? m.press_keys() : value ? `${m.global_shortcut()} ${value}` : m.set_global_shortcut()}
          className="recorder"
          data-empty={value ? undefined : "true"}
          data-recording={recording ? "true" : undefined}
          onBlur={() => setRecording(false)}
          onClick={() => setRecording(true)}
          onKeyDown={onKeyDown}
        >
          {recording ? m.press_keys_ellipsis() : value || m.none()}
        </button>
      </Hint>
      {value && !recording ? (
        <Hint align="end" content={m.clear_shortcut()}>
          <button
            type="button"
            aria-label={m.clear_shortcut()}
            className="plain-btn hover-fill grid place-items-center text-label-3"
            onClick={() => onChange("")}
          >
            <MdiCloseCircle className="size-[var(--icon-row)]" />
          </button>
        </Hint>
      ) : null}
    </span>
  );
}
