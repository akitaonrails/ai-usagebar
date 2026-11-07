---
name: AI Usage popover
description: Explicit visual decisions for the shared tray popover.
---

# Visual contract

The popover's existing theme and geometry tokens are defined in
`windows/popover/src/index.css`. Reuse those variables and the shared
components when adjusting the interface.

## Favorites

Favorited metrics use `--accent`, the blue accent of the current theme, in
both light and dark appearances. Their pressed state is exposed through
`aria-pressed`. Account activation has its own existing indicator.

## App identity

The macOS App Icon menu-bar mode uses the `mdi/gauge` SVG displayed in About,
as an 18pt template image that follows the menu bar's light/dark ink. Keep the
About tile's existing accent treatment. Selecting App Icon preserves favorites.

Start at Login identifies the host platform: Apple on macOS, Windows on
Windows, and a generic login symbol elsewhere.

## Provider headers

Dashboard headers and their tooltips use the provider's catalog name, including
when only one named account is present. Account labels stay in account tabs
and account management. Keep account IDs and stored preferences unchanged.
Account tabs share the existing visible outline used by native provider tabs.
