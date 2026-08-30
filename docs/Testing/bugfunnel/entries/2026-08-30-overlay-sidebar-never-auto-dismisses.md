---
id: 2026-08-30-overlay-sidebar-never-auto-dismisses
date: 2026-08-30
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  The phone's overlay left sidebar stays up after the user taps a page in it,
  because its auto-close hook only runs from the root-layout signal pump and
  navigation wakes no pump at all.
---

## Bug

Reported by Martin dogfooding on his phone: the left sidebar should collapse by
itself when he taps a link inside it, and when he taps outside it. Neither
happened — the sidebar covers the page the tap just opened, and the only way
back to the content is to find the toggle handle again.

## Root cause

`AppModel::close_overlay_drawers` (`frontends/gpui/src/lib.rs`) already did the
right thing: close every `DrawerMode::Overlay` drawer in the resolved tree. It
was called from exactly one place — inside `spawn_root_layout_signal`'s
`for_each`.

That pump is driven by `ReactiveEngine::watch_signal`, which fires on the root
block's data and on `viewport_generation`. Navigation bumps neither:
`UiState::set_focus` (`crates/holon-frontend/src/reactive.rs`) deliberately
bumps no generation — re-interpreting on focus would recreate every editor —
and the `navigation.focus` mirror bumps only `main_nav_generation` /
`main_view_generation`. So a page tap in the sidebar wakes no loop that calls
the hook. The hook was reachable only by coincidence, when something unrelated
happened to fire the root signal in the same window.

Dismissing by tapping beside the drawer had no implementation at all: the
overlay drawers were anchored into `columns::render`'s container with nothing
between them and the page.

## Missing piece

- The auto-close lives entirely in the GPUI window's signal pumps. The headless
  keystone never constructs a window, so the code path does not execute in its
  wiring and no transition sequence could have reached the defect.
  **ENVIRONMENT.**
- No windowed rung had ever driven a drawer at a narrow breakpoint. The windowed
  tests keep whatever size the platform hands them, so every one of them ran the
  wide `if_space` branch where both sidebars are `Shrink` and the overlay code is
  dead. **COVERAGE (secondary).**

## Remedy

FIXED.

- `spawn_overlay_drawer_close` gives the window a pump of its own on
  `UiState::main_view_generation` (exposed as `main_view_signal`), so a move of
  the main region's cursor runs the hook. It is keyed on that counter rather
  than on the focused block because placing a caret also writes the focus: a
  focus-keyed drawer slams shut while the user is typing in the page beside it.
- The pump closes only when the page actually changed, on either of two
  witnesses: `main_nav_generation` moved (`focus` / `open_tab` / `go_home` name
  a target, so they navigate by construction), or the resolved Main view root
  moved. The second witness is the discipline `main_view_generation`'s own
  contract asks for — `close` names a row and `focus_pin` carries no region, so
  both bump without knowing whether the cursor went anywhere, and readers must
  compare the resolved root before acting (the rule `breadcrumb::resolve_trail`
  follows). Closing a BACKGROUND tab now leaves an open sidebar alone. The first
  witness is what keeps a deliberate tap on the page already showing dismissing
  the sidebar even though the root does not move. The pump's first observation
  seeds and never closes, so the signal's opening emission on rebind is a no-op.
  The root read crosses to the tokio runtime the way `resolve_breadcrumb` does;
  a failure to read it is logged at ERROR and leaves open drawers alone.
- `columns::render` draws a scrim over the page still exposed beside an open
  overlay drawer. Pressing it closes every open overlay drawer and stops the
  press, so the row underneath does not also take the caret — what Material's
  scrim and iOS's drawer dismissal both do. Its inset per side is the larger of
  the open overlay width and the layout width the in-flow (`Shrink`) drawers
  occupy, so it covers only what an overlay actually overlays. That second term
  matters at 600..1000px, where the bundled layout renders the left sidebar in
  flow and only the right one floating: without it the scrim spans the in-flow
  sidebar and swallows its clicks. Shrink-only layouts draw no scrim, so desktop
  is untouched.
- The scrim registers bounds under `geometry::OVERLAY_SCRIM_ID`, so a
  geometry-based rung can see where the dismiss area actually is instead of
  inferring it from drawer widths.
- `RebindHandle::drawers` reports the mode each drawer resolved to.
- `frontends/gpui/tests/overlay_sidebar_dismisses_windowed.rs` pins the phone
  (412x915), mid-band (900x900) and desktop (1440x900) layouts. It sets the
  window size through `HOLON_INITIAL_WINDOW_SIZE` so the breakpoint arrives the
  way production delivers it, through `observe_window_bounds` — a viewport
  pushed straight into `UiState` moves the breakpoint without moving any
  geometry, and the scrim bug is invisible to it. A fourth rung drives the
  navigation ops themselves: `go_back` (a page change that names no target),
  a background-tab `close`, and a re-selection of the page already showing.

  Red logs, each failing on its own claim:
  - `lane-logs/RED-overlay-sidebar-1-link-tap.log` — no pump: `left_open=true`
    after navigating to `block:parent`.
  - `lane-logs/RED-overlay-sidebar-2-no-dismiss-area.log` — pump only, no
    scrim: "an open overlay drawer must offer a dismiss area beside it".
  - `lane-logs/RED-overlay-sidebar-3-scrim-over-inflow-sidebar.log` — scrim
    inset by overlay drawers only: at 900px a click inside the left in-flow
    sidebar neither reaches its toggle nor spares the right drawer
    (`left_open=true right_open=false`).
  - `lane-logs/RED-overlay-sidebar-4-focus-keyed-pump.log` — pump keyed on the
    focused block: a right-region pin moves the focus and the open sidebar
    closes (`right_open=false`).
  - `lane-logs/RED-overlay-sidebar-5-background-tab-close.log` — pump with no
    page-change guard: closing a background tab leaves the view on
    `block:parent` and dismisses the sidebar anyway (`root parent -> parent
    left_open=false`), while the `go_back` leg in the same run stays green.

  Green: `lane-logs/GREEN-overlay-sidebar-dismiss.log`.

## Residual

An overlay drawer anchored to the same side as an in-flow drawer would float
over that drawer. The scrim would then start past both rather than at the
overlay's inner edge, so the strip where the overlay overlaps the IN-FLOW
SIDEBAR would not dismiss — that strip is sidebar, not page, so the cost is a
dismiss area shorter than it could be, never a swallowed click on either panel.
The bundled `layout_dsl` never emits that arrangement.
