/**
 * Callback ref that lets a plain vertical mouse wheel scroll a horizontal strip (the Native tab
 * rows) while it overflows. Trackpads are left alone: they already scroll sideways, and their
 * vertical swipe keeps scrolling the page. The listener is native and non-passive because
 * React's `onWheel` cannot `preventDefault`, and the page must not scroll vertically under a
 * strip that consumed the wheel. React 19 runs the returned cleanup when the strip unmounts, so
 * a strip that mounts later is still covered.
 */
export function wheelScrollRef(strip: HTMLElement | null) {
  if (!strip) return;
  function onWheel(event: WheelEvent) {
    if (!strip || strip.scrollWidth <= strip.clientWidth) return;
    if (Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
    // A mouse wheel reports whole notches (WebKit and Chromium both expose them as multiples of
    // 120 in `wheelDeltaY`); a trackpad's vertical swipe does not, and must scroll the page.
    const notches = (event as WheelEvent & { wheelDeltaY?: number }).wheelDeltaY;
    const mouseWheel = notches === undefined ? event.deltaMode !== WheelEvent.DOM_DELTA_PIXEL : notches !== 0 && notches % 120 === 0;
    if (!mouseWheel) return;
    event.preventDefault();
    strip.scrollLeft += event.deltaY;
  }
  strip.addEventListener("wheel", onWheel, { passive: false });
  return () => strip.removeEventListener("wheel", onWheel);
}
