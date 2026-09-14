---
id: 2026-09-14-a-windowed-resize-never-reached-the-window-so-width-sweeps-measured-one-layout
date: 2026-09-14
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  On the headless platform `Window::resize` changed nothing a test could
  observe, so a windowed rung that swept viewport widths measured the boot
  layout once per width and passed without judging any of the widths it named.
---

## Bug

Found while building the width sweep for
`2026-09-12-the-settings-integrations-table-collapses-at-the-minimum-window-width`.
The sweep resized one window through seven widths and asserted the table's
geometry at each. It reported the same geometry seven times: after asking for
360px the window still answered 300px, the width it booted at.

A sweep in that state is worse than no sweep. Every assertion still runs, every
assertion still passes, and the report names seven viewports none of which were
ever laid out. The collapse this fleet is fixing happens only below ~640px, so
a sweep anchored at the boot width would have reported the bug fixed at widths
it never visited.

## Missing piece

`TestWindow::resize` (gpui `crates/gpui/src/platform/test/window.rs:142`) stores
the new bounds and returns. It does not fire the platform resize callback, so
`Window` never re-reads the platform and `Window::viewport_size`
(`crates/gpui/src/window.rs:1997`) keeps serving the cached value it was built
with — and no relayout is scheduled. `TestWindow::simulate_resize`
(`window.rs:90`) does both, but it is reachable only from gpui's own
`TestAppContext`, not from an `AnyWindowHandle` over a `HeadlessAppContext`,
which is what this fleet's windowed rungs hold.

Nothing in the fleet could have caught it: before this lane no windowed rung had
ever resized a window. Every rung that cared about a viewport booted a fresh app
at that size through `HOLON_INITIAL_WINDOW_SIZE`, which works, at the cost of a
full boot per width.

## Remedy

`pbt_harness::windowed_wide::resize_window` pairs `Window::resize` with
`Window::bounds_changed` — the public re-read gpui documents as "exposed
publicly for test infrastructure" — and returns the width the window reports
back, so a caller that wants to judge rather than panic can.

`frontends/gpui/tests/windowed_resize_takes_effect.rs` pins both halves:

- `resize_window_reaches_the_window_at_every_swept_width` — the window reports
  each requested width AND a render pass actually ran at it, so a window that
  answers from fresh bounds without re-laying out cannot pass.
- `a_bare_resize_is_inert_which_is_why_the_helper_exists` — the bare
  `Window::resize` still leaves the viewport at the boot width. It fails the day
  the platform grows the callback, and its message says to delete the pairing
  and the test.

`settings_integrations_table_fits_windowed` and
`settings_introduced_row_fits_windowed` both sweep through the helper. Both
carry their own "the sweep really swept" assertion as well: the narrow end and
the wide end must paint measurably different geometry, so a future regression in
the helper reds the sweeps too rather than quietly emptying them.
