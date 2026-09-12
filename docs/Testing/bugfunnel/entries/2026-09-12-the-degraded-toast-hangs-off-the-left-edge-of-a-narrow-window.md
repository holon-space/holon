---
id: 2026-09-12-the-degraded-toast-hangs-off-the-left-edge-of-a-narrow-window
date: 2026-09-12
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  In a 300-point-wide window the degraded-disclosure toast keeps its full width
  and is anchored right, so it runs off the left edge and the first words of
  every line — including "Secrets are not being saved" — are never painted.
---

## Bug

Found by the `user-connections` dogfood RE-CHECK of 2026-09-12, driving the
real GPUI app at `main` `060022da56b6`.

Booted at `HOLON_INITIAL_WINDOW_SIZE=300x600` — 300 is `MIN_WIDTH` in
`frontends/gpui/src/window_state.rs:24`, the app's own floor, and the window
opened at it. The in-memory-secrets disclosure toast paints with its left side
outside the window. Every line loses its head:

```
eing saved — Secrets are held in memory for
SECRETS_BACKEND=memory). Nothing
ntial field is saved, and every stored secret
xits. Unset HOLON_SECRETS_BACKEND to
ain. 1 seeded fixture secrets were
ate/var/folders/hc/2q6czxpx6j9_87bq…
```

The headline the toast exists to deliver — `Secrets are not being saved` — is
among the lost text, along with the key icon. What remains reads as a fragment
rather than as a warning.

Measured on the capture: the yellow box's right edge sits at x=563 of a 600-px
(2x) image and its painted text begins at x=0, so the box origin is negative.
The toast is right-anchored at a fixed width and the window is narrower than
that width.

The same window makes the Settings integrations table unreadable, recorded
separately as
`2026-09-12-the-settings-integrations-table-collapses-at-the-minimum-window-width`.

Evidence under `scratchpad/dogfood-uc-recheck/shots/`:

- `C-00-boot.png` — the toast at 300x600, first words gone.
- `C-01-table-300.png`, `C-02-fixturebox-300.png` — the same toast over the
  open Settings modal.
- `A-01-boot.png` — the same toast at 1400x900, whole, for comparison.

## Root cause

Not root-caused to a line. The toast's width is not a function of the viewport:
at 1400 and at 800 it paints identically, and at 300 it keeps that size and
overflows. A minimum width with no maximum-against-viewport clamp, combined
with right anchoring, produces exactly this.

The clipping is at the window edge rather than at the toast's own bounds, so
the text is not merely cut — it is drawn outside the surface.

## Missing piece

`refusal_toasts_reach_the_user_windowed.rs` judges toast text at 1400x860 and
1400x720. Every windowed rung that touches toasts uses a wide window; nothing
in the tree opens one near `MIN_WIDTH`, so "a toast is inside the viewport"
has only ever been asserted where the viewport is generous. The one narrow rung
that exists, `integrations_row_narrow_window_windowed.rs`, uses 800x900 and
judges a table row, not a toast.

## Remedy

Open. Clamp the toast's width to the viewport (minus its inset) and let it wrap
further, then extend the toast rung with a case at `MIN_WIDTH` asserting that
every painted toast line's left edge is inside the window.
