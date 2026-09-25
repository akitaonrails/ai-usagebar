/**
 * Whether the user last used a pointer or the keyboard. A menu or popover closed with the mouse
 * should not hand focus back to its trigger: WebView2 then draws the focus ring on it, as if the
 * user had tabbed there. From the keyboard the ring is what tells them where focus went, so it
 * stays.
 */
let lastInput: "keyboard" | "pointer" = "pointer";

if (typeof window !== "undefined") {
  window.addEventListener("pointerdown", () => (lastInput = "pointer"), true);
  window.addEventListener("keydown", () => (lastInput = "keyboard"), true);
}

/** `onCloseAutoFocus` for Radix menus and popovers: skip the focus return after a pointer close. */
export function skipPointerCloseFocus(event: Event) {
  if (lastInput === "pointer") event.preventDefault();
}
