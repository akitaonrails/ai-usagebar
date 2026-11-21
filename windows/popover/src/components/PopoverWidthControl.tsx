import { useCallback, useEffect, useRef, useState, type PointerEvent } from "react";
import MdiChevronDown from "~icons/mdi/chevron-down";
import MdiChevronUp from "~icons/mdi/chevron-up";
import MdiRestore from "~icons/mdi/restore";
import { Hint } from "@/components/Hint";
import { m } from "@/paraglide/messages.js";

interface PopoverWidthControlProps {
  /** `[tray] width` as the host holds it; null is the style's own width. */
  value: number | null;
  /** The active style's own width on this host, in logical points. */
  original: number;
  /** Why the host refused the last change, or empty. */
  error: string;
  onChange: (width: number | null) => void;
}

/** Wait before a held arrow starts repeating, then the pause between repeats. */
const HOLD_DELAY_MS = 300;
const HOLD_INTERVAL_MS = 100;
/** A held arrow moves 1 point per repeat at first and reaches this many after the ramp. */
const HOLD_MAX_STEP = 5;
const HOLD_RAMP_MS = 3000;

interface Hold {
  timer: number;
  started: number;
  target: HTMLButtonElement;
  pointerId: number;
}

/**
 * Popover width in logical points, one control for both styles: a reset arrow, the number, and
 * two step arrows. The range is three quarters to three times `original`, the same limits the host
 * enforces. Typing commits on Enter or blur (Escape reverts); the arrow keys and a click move
 * one point; holding a step arrow repeats and accelerates. Only a whole number inside the range
 * is ever sent, and reset sends null so the host removes the key.
 */
export function PopoverWidthControl({ value, original, error, onChange }: PopoverWidthControlProps) {
  const minimum = Math.ceil(original * 0.75);
  const maximum = Math.floor(original * 3);
  const clamp = (width: number) => Math.max(minimum, Math.min(maximum, width));
  const width = clamp(value ?? original);
  const [draft, setDraft] = useState(String(width));
  // The same text as `draft`, readable from a hold timer between renders.
  const draftRef = useRef(draft);
  // The last width sent that the host has not echoed yet; undefined when nothing is pending.
  const pending = useRef<number | null | undefined>(undefined);
  const previousOriginal = useRef(original);
  const revertOnBlur = useRef(false);
  const hold = useRef<Hold | null>(null);
  const step = useRef<(amount: number) => void>(() => {});

  function showDraft(text: string) {
    draftRef.current = text;
    setDraft(text);
  }

  const stopHolding = useCallback(() => {
    const active = hold.current;
    hold.current = null;
    if (!active) return;
    window.clearTimeout(active.timer);
    if (active.target.hasPointerCapture(active.pointerId)) active.target.releasePointerCapture(active.pointerId);
  }, []);

  // A hold ends with the press, and with anything that takes the pointer or the window away.
  useEffect(() => {
    const onHidden = () => { if (document.hidden) stopHolding(); };
    const onEscape = (event: KeyboardEvent) => { if (event.key === "Escape") stopHolding(); };
    window.addEventListener("pointerup", stopHolding, true);
    window.addEventListener("pointercancel", stopHolding, true);
    window.addEventListener("blur", stopHolding);
    window.addEventListener("contextmenu", stopHolding);
    window.addEventListener("keydown", onEscape);
    document.addEventListener("visibilitychange", onHidden);
    return () => {
      stopHolding();
      window.removeEventListener("pointerup", stopHolding, true);
      window.removeEventListener("pointercancel", stopHolding, true);
      window.removeEventListener("blur", stopHolding);
      window.removeEventListener("contextmenu", stopHolding);
      window.removeEventListener("keydown", onEscape);
      document.removeEventListener("visibilitychange", onHidden);
    };
  }, [stopHolding]);

  // So does a style change (new limits) or a refused save.
  useEffect(stopHolding, [original, error, stopHolding]);

  // Follow the host's value, except while it is still echoing an older step of a run: showing
  // each echo would make the number jump back under a held arrow.
  useEffect(() => {
    const settled = pending.current === undefined || pending.current === value;
    if (original !== previousOriginal.current || error || settled) {
      previousOriginal.current = original;
      pending.current = undefined;
      showDraft(String(width));
    }
  }, [value, width, original, error]);

  function send(next: number | null) {
    pending.current = next;
    // After a refusal the number stays on what is applied until the host accepts a change.
    showDraft(String(error ? width : (next ?? original)));
    onChange(next);
  }

  /** The draft as a width inside the range; anything but a whole number is the applied width. */
  function draftWidth() {
    const text = draftRef.current.trim();
    return /^\d+$/.test(text) && Number.isSafeInteger(Number(text)) ? clamp(Number(text)) : width;
  }

  step.current = (amount) => {
    const current = draftWidth();
    const next = clamp(current + amount);
    if (next !== current) send(next);
    if (next === minimum || next === maximum) stopHolding();
  };

  function startHolding(event: PointerEvent<HTMLButtonElement>, direction: 1 | -1) {
    if (event.button !== 0 || !event.isPrimary) return;
    event.preventDefault();
    stopHolding();
    const active: Hold = {
      timer: 0,
      started: performance.now(),
      target: event.currentTarget,
      pointerId: event.pointerId,
    };
    hold.current = active;
    // Captured, so releasing outside the button still ends the hold.
    active.target.setPointerCapture(active.pointerId);
    step.current(direction);
    if (error) {
      stopHolding();
      return;
    }
    const repeat = () => {
      if (hold.current !== active) return;
      const ramp = (performance.now() - active.started) / HOLD_RAMP_MS;
      const points = Math.min(HOLD_MAX_STEP, 1 + Math.floor((HOLD_MAX_STEP - 1) * ramp));
      step.current(direction * points);
      if (hold.current === active) active.timer = window.setTimeout(repeat, HOLD_INTERVAL_MS);
    };
    if (hold.current === active) active.timer = window.setTimeout(repeat, HOLD_DELAY_MS);
  }

  function commitDraft() {
    if (revertOnBlur.current) {
      revertOnBlur.current = false;
      return;
    }
    const next = draftWidth();
    if (next !== width) send(next);
    else showDraft(String(width));
  }

  const shown = /^\d+$/.test(draft) ? clamp(Number(draft)) : width;
  const atOriginal = value === null && pending.current == null && draft === String(width);

  function stepButton(direction: 1 | -1) {
    const label = direction > 0 ? m.increase_popover_width() : m.decrease_popover_width();
    return (
      <Hint align="end" content={label}>
        <button
          type="button"
          aria-label={label}
          disabled={direction > 0 ? shown >= maximum : shown <= minimum}
          onPointerDown={(event) => startHolding(event, direction)}
          onLostPointerCapture={stopHolding}
          // A pointer press already stepped on pointerdown; `detail` is 0 only for Enter/Space.
          onClick={(event) => { if (event.detail === 0) step.current(direction); }}
        >
          {direction > 0 ? <MdiChevronUp aria-hidden="true" /> : <MdiChevronDown aria-hidden="true" />}
        </button>
      </Hint>
    );
  }

  return (
    <div
      className="popover-width-control"
      role="group"
      aria-label={m.popover_width()}
      aria-invalid={error ? true : undefined}
    >
      <Hint align="end" content={m.restore_popover_width()}>
        <button
          type="button"
          className="popover-width-reset"
          aria-label={m.restore_popover_width()}
          disabled={atOriginal}
          onPointerDown={(event) => { stopHolding(); event.preventDefault(); }}
          onClick={() => send(null)}
        >
          <MdiRestore aria-hidden="true" />
        </button>
      </Hint>
      <input
        type="text"
        inputMode="numeric"
        role="spinbutton"
        aria-label={m.popover_width()}
        aria-valuemin={minimum}
        aria-valuemax={maximum}
        aria-valuenow={shown}
        maxLength={5}
        value={draft}
        onChange={(event) => showDraft(event.target.value)}
        onFocus={stopHolding}
        onBlur={commitDraft}
        onKeyDown={(event) => {
          if (event.key === "ArrowUp" || event.key === "ArrowDown") {
            event.preventDefault();
            step.current(event.key === "ArrowUp" ? 1 : -1);
          } else if (event.key === "Enter") {
            event.preventDefault();
            event.currentTarget.blur();
          } else if (event.key === "Escape") {
            // Handled here so Escape gives up the edit instead of leaving Settings.
            event.preventDefault();
            revertOnBlur.current = true;
            showDraft(String(width));
            event.currentTarget.blur();
          }
        }}
      />
      <div className="popover-width-steps">
        {stepButton(1)}
        {stepButton(-1)}
      </div>
    </div>
  );
}
