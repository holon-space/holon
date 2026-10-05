---
id: 2026-10-06-a-shopping-item-active-and-ticked-at-once-never-converges-after-an-edit
date: 2026-10-06
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A name the app holds both active and ticked pulled as one ticked row with the
  active amount and no refusal; a Holon amount edit then went to the ticked
  entry and every round failed after two commits.
---

## Bug
Found by a fresh-context verifier running the shipped pull mapping by hand
(`lane-logs/shopping-edits-verify.md` D3, 2026-10-06). A probe replay showed
every round failing with "1 change(s) still un-pushed after 2 commits"
(`lane-logs/a4-d3probe-remote-list-an-item-active-and-ticked-at-once-is-refused-and-still-converges.log`).

## Root cause
The pull mapping folded the two entries into one row with `checked: true` and
the active amount. The update mapping keys off the peer's `checked`, so the
edit replaced the ticked entry while the pull kept reading the active one.

## Missing piece
COVERAGE: no generator arm built the state; `Add` always drew a fresh name and
`AddDuplicate` only duplicated active rows.

## Remedy
The pull mirrors the active entry and reports the ticked one under
`shopping_refusal` by name. New arm `AddActiveOverTicked` and replay case
`remote-list-an-item-active-and-ticked-at-once-is-refused-and-still-converges`.
Red: `lane-logs/a4-RED-remote-list-an-item-active-and-ticked-at-once-is-refused-and-still-converges.log`.
