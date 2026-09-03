---
id: 2026-09-03-settings-modal-clips-its-own-controls-with-no-way-to-scroll
date: 2026-09-03
gap: PERCEPTION
secondary: COVERAGE
status: OPEN
summary: >-
  The Settings modal is shorter than its content, so the integrations table is
  cut off mid-row and the rows below it cannot be reached by any means.
---

## Bug

Found by the `dogfood-search` lane, opening Settings through the toolbar gear
on a 1512x948pt window.

The modal renders Appearance, Integrations (preferences), Data, and then the
integrations control table. The table is cut off inside the first data row: the
`claude-history` row is complete, the next row shows only the top of the word
`unconfigure` and nothing below it is visible. There is no scrollbar, no
resize handle, and a wheel scroll dispatched over the modal changes nothing
(verified: identical screenshot before and after `scroll dy=5` at the modal's
centre).

Every bundled integration below the first is therefore unreachable — the modal
is described in the source as "the CONTROL surface — every bundled provider
with a switch", and it can only operate the first one.

Two smaller defects in the same view: the `Config` cell wraps `unconfigured`
into `unconfigure` + `d`, and the `Setup` column's second control renders as
`OP` above `Open`, i.e. the label is being clipped and wrapped rather than
laid out.

## Root cause

Not diagnosed beyond the symptom. The modal's content column
(`frontends/gpui/src/lib.rs:1074-1132`, `modal_overlay`) stacks the rendered
preference section, the integrations section and the build stamp into a plain
flex column with no scroll container and no max-height/overflow policy, so the
modal's own height is what decides how much of the column a user can see.

## Missing piece

No windowed test opens Settings and asserts its content is reachable. The
comment on the gear's bounds tracker says as much — the rect exists precisely
"so a window test reaches anything the modal paints" — but no test uses it,
and nothing bounds the section's height against the modal's.

A headless assertion cannot express this: the view model contains every row
whether or not the window paints it, which is exactly why the previous dogfood
run reported the toolbar as unreachable and never saw the modal at all.

## Remedy

OPEN. Give the modal body an `overflow_y_scroll` container with a max height
tied to the window, and fix the two column labels. Pin with a windowed test
that opens Settings via the tracked gear rect and asserts every integration row
has a painted rect with non-zero area.

Method note for the next run: the toolbar IS drivable. `click {x, y}` at the
gear's window coordinates opens the modal (it answers `handled:false` while
having handled it), and `send_key_chord` with `cmd`+`k` on a *rendered
editable block* opens quick-open. That closes
`2026-09-03-titlebar-toolbar-is-invisible-to-describe-ui` as a driving
obstacle, though the describe_ui blindness it names is real and unchanged.
