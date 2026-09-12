---
id: 2026-09-12-only-the-first-settings-integrations-row-can-be-clicked
date: 2026-09-12
gap: FALSE-ALARM
secondary: null
status: NOTED
summary: >-
  Reported as "only the first Settings > Integrations row is reachable and the
  modal does not scroll"; measurement refutes it — the modal scrolls and every
  row is clickable, and the probe's wheel event was dropped because it carried
  no pointer move.
---

## Bug

Reported by a verifier driving the real windowed app for the
`sidecar-sync-fixes` lane. Two observations were offered:

1. In a 1512x900 window the six bundled rows' `Enabled` cells sit at
   y = 841 / 897 / 953 / 1009 / 1065 / 1121 with heights 20 / 3 / 0 / 0 / 0 / 0
   — everything after the first row collapsed and/or below the window bottom.
   A real click at the centre of the last row left the `integration_state`
   mirror unchanged.
2. A ten-line scroll-wheel event over the modal changed no coordinate, so the
   modal "does not scroll either".

The stakes are why it was raised: this lane's whole remedy for a bad connection
file is "switch it off in Settings > Integrations". If only the first row can be
clicked, that remedy does not exist for any other connection.

## Root cause

**Observation 1 is real and is not a defect. Observation 2 is a measurement
artifact, and it is the one the conclusion rested on.**

Reproduced independently in the same harness at the same window size
(`lane-logs/item10-red-20260912-062634.log`, the geometry block): the six cells
sit at y = 750 / 804 / 858 / 912 / 966 / 1020, heights 20 / 6 / 0 / 0 / 0 / 0.
The absolute offsets differ from the report by 91px — a different scroll or
content state — but the shape is identical.

Those heights are **clip**, not collapse.
`frontends/gpui/src/geometry.rs:318` records every tracked element's bounds
already intersected with `window.content_mask()`, precisely because gpui
hit-tests the same intersection. The modal panel
(`frontends/gpui/src/lib.rs:711-716`) is `max_h(720px)`, centred in a 900px
window, so it clips at y=810 — and 810 is exactly where the second row's cell
stops (804..810, h=6). Rows three to six are clipped away entirely, which is
what `h=0` means.

The panel already declares `.overflow_y_scroll()` on the same lines. It works.
Driving it with a wheel gesture moves all six rows into view:

```
before:  todoist cell y=1020.0..1020.0 h= 0.0   (clipped away)
after 4 notches of 60px: todoist cell y= 780.0.. 800.0 h=20.0
```

The reason the earlier probe saw no movement is in gpui itself:
`Interactivity::paint` registers the wheel handler behind
`hitbox.should_handle_scroll(window)` (`crates/gpui/src/elements/div.rs:2693`),
which asks whether the pointer is hovering that hitbox. A `ScrollWheelEvent`
dispatched with no preceding `MouseMoveEvent` leaves the window's pointer
position wherever it was, the modal's hitbox is not hovered, and the event is
dropped in silence. A real trackpad or mouse always moves the pointer first, so
no user can be in that state.

With the scroll performed, a real click on the LAST row's switch flips
`enabled` for `todoist` in the `integration_state` mirror. The remedy this lane
depends on exists for every row.

## Missing piece

Nothing in the product. What was missing is on the test side, twice over:

- **The probe's gesture was not a gesture.** A bare `ScrollWheelEvent` is not
  what a pointing device sends, and the harness gave no signal that the event
  had been discarded — so "no coordinate changed" read as "the modal cannot
  scroll" rather than "the modal was never asked to".
- **No rung asked whether a row below the fold can be operated.**
  `settings_integrations_table_fits_windowed` judges the table's HORIZONTAL fit
  and `settings_integrations_ops_windowed` clicks one op button on the `gcal`
  row, which is above the fold. Neither would have contradicted the report, so
  there was nothing to check it against.

## Remedy

`gap: FALSE-ALARM` — no product defect, so this is counted on its own line and
excluded from the four-class escape distribution. Recorded rather than dropped,
because the refutation is the useful artifact: the next agent measuring a gpui
scroll surface needs to know that the pointer move is part of the gesture.

**The rung**:
`frontends/gpui/tests/settings_integrations_last_row_toggle_windowed.rs` — opens
the modal through the toolbar gear, dumps the `Enabled` column's geometry both
before and after scrolling, wheels over the panel until the LAST row's switch
is clickable (bounded at 20 notches, so a modal that genuinely does not scroll
fails instead of hanging), clicks it, and asserts on STATE: that provider's
`enabled` flipped in the `integration_state` mirror and no other provider's
did. Non-vacuity: at least two rows, the painted row count equals the mirror's,
and the row clicked is the last in the mirror's `provider_name ASC` order.

Its first form asserted the last row is on screen with NO scrolling. That went
red for a true reason (`the last row's switch state_toggle#98 paints 36.0x0.0
px — it is collapsed to nothing, so no click can land on it`), but the oracle
was too strong: the table is taller than any modal that fits a 900px window, so
requiring no scroll would fail forever and for no defect. The delivered rung
asserts what a user actually has.

**Left open, deliberately, as taste rather than defect** — the modal caps
itself at 720px inside a viewport that offers 868px, and gpui paints no
scrollbar, so the content below the fold has no visible affordance. Neither
was measured as breaking anything, and neither is fixed here.

**Unproven.** Every number above comes from ONE window size (1512x900) in the
windowed harness, driven by synthetic input. Whether a user's resizable shipped
window behaves the same is not established by this entry: a shorter window
clips more rows, a taller one may clip none, and a real trackpad's momentum
scrolling and a real compositor's hit-testing are not what
`HeadlessAppContext::dispatch_event` exercises. What would settle it: run the
rung across a spread of window heights (`HOLON_INITIAL_WINDOW_SIZE`) including
one shorter than the modal's minimum content, and a `dogfood-explorer` pass on
the real app that resizes the window and scrolls the modal with the real
trackpad before clicking the bottom row.
