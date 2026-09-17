---
id: 2026-09-14-the-settings-modal-clips-its-lower-sections-at-the-minimum-window-width
date: 2026-09-14
gap: COVERAGE
secondary: PERCEPTION
status: OPEN
summary: >-
  At 360px the Settings modal's content is taller than the panel, the panel does
  not scroll to the rest, and the integrations table's rows cannot be reached at
  all — only its header row is visible.
---

## Bug

Found during the `perception-fixes` live-verify pass, driving the real GPUI app
through its MCP server at 360x900 and again at 360x1400.

The Settings modal opens centred with a capped height. Below the Appearance and
Data sections the integrations table begins, and at 360px its header row breaks
across two lines (correctly — that is the wrap the column floors install). The
panel ends there. Every data row sits below the panel's lower edge.

Wheel events dispatched inside the panel through the MCP `scroll` tool report a
delta of `[0.0, 0.0]` at three different points in the body, and the painted
content does not move. Making the WINDOW taller does not help: the panel keeps
its own cap, so the fold stays in the same place relative to the content.

The effect is that at the app's own minimum width a user can see that there is
an integrations table and cannot reach a single row of it — cannot read a
status, cannot reach a toggle, cannot press a Setup button.

Screenshots in `lane-logs/shots/` of the `perception-fixes` workspace:
`04-narrow-settings.png` (360x900), `06-narrow-tall-settings.png` and
`07-narrow-table-rows.png` (360x1400, after three scroll attempts).

## Missing piece

No rung asserts that the Settings modal's sections are REACHABLE — only that
what is painted is laid out correctly. `settings_integrations_table_fits_windowed`
sweeps 300..1512 and deliberately tolerates a narrow run painting fewer rows,
with the comment that "below the wrap threshold a row occupies several lines and
the panel scrolls". That assumption is what the live pass contradicts: the panel
does not scroll to them. The sweep stays green because its claims are about the
cells it CAN see.

The claim that is missing is reachability: at every swept width, the last row of
the last section must be reachable — visible, or brought into view by a bounded
number of wheel notches over the panel. That is a different shape from "what is
painted fits", and nothing in the windowed fleet makes it.

## Remedy

Not fixed here — out of this lane's scope, and it needs its own covering rung
first. Suggested order: extend the width sweep with a reachability claim (scroll
the panel to its end and require the final row to register bounds), show it red
at 360, then fix whichever of the modal's height cap or its scroll region is
wrong.

Worth noting for whoever takes it: this is not a regression from the column
floors. The floors change how the table wraps, which changes how far down the
rows sit, but the panel's fold and its scrolling are independent of them.

## Remedy (2026-09-17, `fix-settings-modal-rows` lane)

The height half is fixed. `modal_overlay` now takes a `ModalHeight` and the
Settings modal takes `FillAvailable`, so the panel's height comes from the
window (`h_full`, ceiling 900px) instead of the fixed 720px cap. That cap put
the panel's lower edge at y=810 in a 900px window, and the content above the
table — the preferences form, then the Integrations heading and its
next-launch notice — already reached it: one more preference field (the ICS
connector's `ics.calendar_url`) left the integrations table with a single row's
top sliver above the fold, which turned five windowed rungs red. Verified red
and green at 1512x900 in `frontends/gpui/src/lib.rs`'s `ModalHeight`.

Two corrections this lane measured, worth keeping:

- The panel DOES scroll. The 2026-09-12 refutation
  (`2026-09-12-only-the-first-settings-integrations-row-can-be-clicked.md`) and
  `settings_integrations_last_row_toggle_windowed` both hold: a wheel over the
  panel reaches the bottom rows. A cap therefore costs FIRST-PAINT visibility,
  not reachability — which is what the five rungs above actually judge.
- The rungs' visibility oracle clips to the panel, and a row clipped to a
  sliver still counts as visible. So the table sat at the fold for months with
  the rungs green and no slack, and nothing said so.

Still owed, exactly as asked below: a rung that scrolls the panel to its end
and requires the last row to register bounds, at a swept width. Everything the
windowed fleet judges is the state at boot.
