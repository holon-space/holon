---
id: 2026-09-08-engine-focus-dangles-on-deleted-block
date: 2026-09-08
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Deleting the block that holds editor focus can leave engine focused_block
  pointing at a block that no longer exists, with no editor and no keystrokes.
---

## Bug

Found by the verifier on the `quick-open-focus` lane (probe 3d) while probing
for stale-handle hazards around the quick-open overlay.

Sequence: focus a row, open quick-open, delete the focused block, press
Escape. Nothing panics and the modal closes, but `engine.focused_block()` is
still `Some(block:chord-target)` for a block that is gone, `window_focused` is
empty, and keystrokes are dropped.

The overlay is incidental — it is what made the dangle visible, not what
caused it.

## Root cause

NOT root-caused, which is why this stays OPEN rather than being filed as a
false alarm.

The frontend does have a clear: `focus_clear_on_delete_target` /
`clear_focus_after_delete` (`crates/holon-frontend/src/reactive.rs`) run in the
success arm of a delete dispatched through `dispatch_intent`, mirroring the
reference model's `clear_focus_if_deleted`. The open question is whether the
probe's delete travelled that path at all. A delete that reaches the store by
any other route — a direct backend call, CDC from a peer, an org-file ingest —
has no channel back into `UiState`, so the mirror would keep the stale focus.
If that is the mechanism, the defect is real but its home is the missing
out-of-band channel, not the delete op.

## Missing piece

No transition deletes the focused block from OUTSIDE the frontend dispatch
path, so the reference model and the SUT can never disagree about it: both
clear focus on their own dispatched deletes. `inv-window-focus-matches-engine-focus`
does not bite either — engine focus on a row with no mounted editor is its
documented skip.

## Remedy

OPEN. Next step is to establish the deletion route in the reproduction
(dispatched op vs. direct store write). If it is the out-of-band route, the
remedy is a transition that deletes the focused block through the backend and
an invariant asserting `focused_block` names a live block.
