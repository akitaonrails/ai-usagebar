/**
 * Measure the panel's intrinsic height without feeding the native window's
 * current viewport height back into the next resize request.
 */
export function measurePanelHeight(shell) {
  if (!shell || typeof shell.querySelector !== "function") return 0;
  const content = shell.querySelector("[data-scroll-content]");
  const contentHeight = content ? Math.max(content.offsetHeight || 0, content.scrollHeight || 0) : 0;
  let height = 0;
  for (const child of Array.from(shell.children || [])) {
    const isScrollRegion = child.dataset?.scroll !== undefined;
    height += isScrollRegion && content ? contentHeight : child.offsetHeight || 0;
  }
  return Math.ceil(height);
}
