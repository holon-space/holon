---
id: 2026-09-02-slot-birth-leaves-a-stale-ref-editor-that-suppresses-the-chord-click
date: 2026-09-02
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The creation-slot birth moves reference focus to the newborn without closing
  the previously active reference editor, so a later chord op on that stale
  editor block hits `model_chord_click_focus`'s editor arm and skips a click
  production performs — `inv-editor-caret/mirror` reds with the reference one
  full block-length behind the SUT.
---

## Bug

`just keystone-smoke` in the 2026-09-02 land battery failed with ONE signature
the known-reds classifier could not place (`1 novel, 21 known`):

```
reconciled composed sequence diverged from the oracle: [("inv-editor-caret/mirror", "[inv-editor-caret/mirror] Caret mismatch on block:parent: reference model cursor_byte=0, SUT tracked caret=6")]
```

Battery log:
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/land-battery.98400.log:5930`.
It is an INTERMEDIATE shrink candidate, not the reported `minimal failing
input` — proptest's shrink converged onto the registered
`history-join-phantom-row` signature instead, so the log carries no minimal
sequence for this one. The candidate's action trail, read off its
`[inv-sql-budget]` breadcrumbs, was `BulkExternalAdd → SplitBlock →
CreateBlockUnderFocus → Indent` over the drawn wiring
`storage={Loro, Turso} sync={} actors={}`.

It classified NOVEL by design, not by omission: the `editor-caret-mirror` row
in `docs/Testing/KeystoneKnownReds.md` is at status `fixed-pending-soak`, and
`scripts/keystone-known-reds.sh:47` matches `known-red` rows only. The shape
here is NOT the one that row's 2026-08-22 fix closed (a stale armed caret seed
in `headless_editor_mirror.rs`); it is a distinct defect on the same invariant,
and its direction is the exact INVERSE of the class documented at
`crates/holon-integration-tests/src/pbt/transitions/mod.rs:54-62` (that one
reds `ref content.len()` vs `SUT 0`; this one reds `ref 0` vs
`SUT content.len()`).

A deterministic 3-transition reproducer was derived from the mechanism and
confirmed byte-identical, as the SOLE violation, 6/6 runs across two trees.

## Root cause

The reference side is wrong; production behaves correctly.

1. `SplitBlock{block:parent, position: 0}` — at a position-0 split the
   text-bearing lower block IS `block_id` itself, so the reference opens its
   active editor on `block:parent` at caret 0
   (`crates/holon-integration-tests/src/pbt/transitions/split_block.rs:297-318`).
2. `CreateBlockUnderFocus{id: null}` — the creation-slot birth sets
   `ui.tab.focused_block` to the newborn but deliberately does NOT mount a
   reference editor
   (`crates/holon-integration-tests/src/pbt/reference_state.rs:2096-2106`).
   That omission is documented as a known gap on the assumption that both
   sides then sit Unobservable. **That assumption only holds when no editor was
   already open.** Here one was, so the reference's `active_editor` is left
   STALE on `block:parent` while its global focus is the newborn.
3. `Indent{block:parent}` — the chord ref apply calls
   `model_chord_click_focus`
   (`crates/holon-integration-tests/src/pbt/transitions/indent.rs:116`), whose
   skip predicate is
   `global_focused_block == block || active_editor_block == block`
   (`crates/holon-integration-tests/src/pbt/transitions/mod.rs:77-81`). The
   stale editor arm matches, so the reference skips the click and keeps
   caret 0.
4. The SUT's predicate is focus-only —
   `self.engine.focused_block().as_ref() != Some(entity_id)`
   (`crates/holon-frontend/src/user_driver.rs:1233`, and the same expression at
   `:894`) — so it seeds for the click and lands the caret at end-of-text,
   `len("parent") = 6`.

Production is right at step 4: the slot birth mounts a real editor on the
NEWBORN, so by the time the chord fires, `block:parent` is not the open editor
and clicking it does re-open it at end-of-text. Only the reference still thinks
`block:parent` is the active editor.

`mod.rs:44-52` claims the skip predicate is "THE SUT'S, VERBATIM". It is not —
it carries a second arm the SUT has no counterpart for. That arm was added to
close the inverse divergence class; it reads a field the slot birth leaves
stale, which opens this one.

## Missing piece

`birth_block_under_slot` does not close the reference's previously active
editor. Its documented rationale covers only the "both sides absent" case and
does not address a birth that happens while some other block's reference editor
is open, which is when `active_editor` becomes a stale input to
`model_chord_click_focus`.

## Reproducer

Deterministic, 3 transitions, sole violation. Now PINNED in
`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl` as
`slot-birth-closes-the-ref-editor-so-a-later-chord-still-clicks`
(`SplitBlock{block:parent, 0}` → `CreateBlockUnderFocus{id: null}` →
`Indent{block:parent}`, wiring `{Loro, Turso}` — the mirror is deselected in
Loro-only draws, so that draw is load-bearing).

Red-for-the-right-reason log: `lane-logs/red-caret-oracle.log` — the whole
corpus green, this case the only failure, signature verbatim.

Attribution — byte-identical message, sole violation, every run:

| tree | rev | run 1 | run 2 | run 3 |
|---|---|---|---|---|
| main baseline | `50f878cc3824` | RED | RED | RED |
| integration tip (+ subtree-share-race) | `a7452468` | RED | RED | RED |

Logs: `lane-logs/caret-probe-main-{1,2,3}.log` in the `main-baseline`
workspace, `lane-logs/caret-probe-tip-{1,2,3}.log` in `subtree-share-race`.
The defect is PRE-EXISTING on `main` and is not caused by the sharing-race
lane's `holon-loro` changes.

## Remedy

FIXED in `crates/holon-integration-tests/src/pbt/reference_state.rs`
(`birth_block_under_slot`): the birth now commits the previously active editor
if dirty and then CLOSES it, restoring the "both sides absent → Unobservable"
state the function's own comment already assumed. The commit half matches the
other two focus-authority-move sites
(`transitions::model_chord_click_focus`, `FocusEditableText::apply_to_ref`),
so a dirty buffer is not silently dropped by the close.

Deliberately NOT done: mounting a reference editor over the newborn. Prod does
seat a caret there, but opening one reds `inv-editor-text/mirror`, and closing
that needs the transition to type through the editor keystroke sink instead of
dispatching `set_field` — a change to what the transition drives, and its own
decision. The close is strictly weaker and sufficient.

Also deliberately NOT done: a third arm on `model_chord_click_focus`. The
predicate's problem was that it consulted a field the birth left stale, not
which arms it has. Its doc comment previously claimed the predicate was the
SUT's "VERBATIM", which it is not — it has two arms where prod has one. That
comment now says so, and records that the editor arm is sound only while
`active_editor` names a block prod is really editing.
