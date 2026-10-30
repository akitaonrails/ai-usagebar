# KDE plasmoid — future parity with Win/mac tray

The Windows/macOS tray shares the React popover under `windows/popover/`.
This plasmoid is a Plasma readout + Configure UI. Below is what the tray
already has that we may port later. Packaging-only and host-only items
(self-update, Launch at Login, Acrylic skins, macOS menu-bar Chart/Logos)
are omitted on purpose.

## Done / in progress

- [x] Hide + reorder providers via Configure vendor ring (tabs/cards)
- [x] Provider brand icons on tabs and cards (same SVGs as the tray marks)

## Worth considering (best ROI)

1. **Used ↔ Left (“Show Usage As”)** — click % / setting; remaining vs used.
2. **Rich pace + bar tick** — “limit in 1h”, even-burn tick; report may need
   `elapsed_pct` (README currently declines a pace marker).
3. **Usage goal bar** — second bar = even-pace target vs actual.
4. **Reset: countdown ↔ exact time + timeline popover**
5. **Always Visible / On Demand** metric tiers (beyond Configure checkboxes)
6. **Error CTAs** — Refresh / Open TUI on the status surface (Add key / Sign-in
   is larger and needs dialogs + CLI)
7. ~~**Provider brand icons**~~ — done
8. **Star metrics for the compact panel** — user-picked instead of auto top-2
9. **Banked reset-credits UI** as a first-class chip/timeline
10. **Quota threshold notifications** via Plasma (KNotify)

## Partial today

- Provider visibility/order: Configure ring (does not write `enabled` in toml)
- Multi-account: flat ring entries vs tray grouped `AccountTabs`
- Tabs/cards ≈ Classic/Native layout, without logo+% chip chrome

## Probably skip on Plasma

- In-app updater, About-as-product-shell, Quit / Launch at Login
- Global shortcut recorder (use Plasma widget activation)
- Claude Desktop prepare / Keychain / host account switch
- Copy card as image

## Suggested order if picking these up

1. Quick wins: Used↔Left, error Refresh/TUI CTAs  
2. Reading UX: pace + goal + exact reset/timeline  
3. Density: On Demand / panel stars  
4. In-panel auth only with a way to test dialogs end-to-end  
