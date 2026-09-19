import { useEffect, useRef, useState } from "react";

/** How long a click keeps its own label on screen before the host's state takes over again. */
const HOLD_MS = 1200;

/**
 * Optimistic feedback for a command whose result may arrive in a millisecond
 * and look exactly like the state before it (an install that fails the same
 * way twice). The label the click sets stays visible for HOLD_MS, whatever the
 * host says meanwhile, then the real state shows again.
 */
export function useBusyLabel(): [string | null, (label: string) => void] {
  const [busy, setBusy] = useState<string | null>(null);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    };
  }, []);

  function start(label: string) {
    if (timer.current !== null) window.clearTimeout(timer.current);
    setBusy(label);
    timer.current = window.setTimeout(() => {
      timer.current = null;
      setBusy(null);
    }, HOLD_MS);
  }

  return [busy, start];
}
