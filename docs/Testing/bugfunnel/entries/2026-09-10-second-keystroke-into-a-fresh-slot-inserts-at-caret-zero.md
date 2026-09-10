---
id: 2026-09-10-second-keystroke-into-a-fresh-slot-inserts-at-caret-zero
date: 2026-09-10
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  The second character typed into a freshly created creation-slot block is
  inserted at caret offset 0 instead of after the first, so typing "ab"
  silently stores "ba".
---

## Bug

Seat the caret in an empty page's creation slot (quick-open Enter into a
childless page), then type two characters. The block is created correctly and
both characters land, but in the wrong order: typing `ab` yields `ba`.

**Silent corruption, not an error.** Nothing is logged, nothing returns `Err`,
and the SQL row, the Loro container and the rendered editor all agree on the
wrong value. That makes it worse than the loud `Block not found` failure it sits
next to — a user sees only that their typing came out scrambled. Under the
fail-loud priority order in `CLAUDE.md` this is the forbidden case: it degrades
to look fine.

Found by a new windowed PBT written for the `slot-birth` lane (D112), not by
dogfooding — the lane added the first test that ever typed TWO characters into a
fresh slot.

## Root cause

The editor's own input already holds `"ba"` before any write happens. Every
layer below it records faithfully:

```text
DIAG delta buffer="a" new_text="ba"
DIAG cell.apply_text_op op=Insert { pos_codepoint: 0, text: "b" } container_before="a"
```

The writer's buffer is `"a"` — correct, not stale. `compute_text_delta("a",
"ba")` (`crates/holon-core/src/cell.rs:150-174`) correctly yields
`Insert@0 "b"`, and the container correctly becomes `"ba"`.

The caret is seeded at offset 0 when the slot is seated —
`ReactiveEngine::birth_creation_affordance` calls
`set_focus_with_caret(id, 0)` (`crates/holon-frontend/src/reactive.rs`) — and it
is still 0 when the second character arrives. The first character does not
advance it, so the second is inserted at the front.

**PRE-EXISTING.** An A/B against `main` `8c8c564d5890`'s birth code (extracted
with `git -C <primary repo> show`, plus only the test-fixture registry install)
reproduces it identically: `"ba"` on both trees, with
`not found in Loro tree` appearing 0 times on both. It is not the slot-birth
change. Logs: `lane-logs/ab-base-birth-*.log`, `lane-logs/diag-h1-*.log` in the
`slot-birth` workspace.

## Missing piece

Two, which is why this is recorded as dual:

- **ENVIRONMENT (primary).** The failing code path is the GPUI editor's caret /
  `InputState`, which the headless rung does not have — its editor-caret
  invariants deselect there (`inv-editor-caret/mirror` runs windowed-only). The
  keystone can generate the interaction; the code that gets it wrong does not
  exist in the headless wiring.
- **COVERAGE (secondary).** No test on ANY rung had typed two characters into a
  fresh slot. Every existing case typed exactly one, which cannot distinguish a
  caret that advances from one that does not.

## Remedy

OPEN. Not fixed in the `slot-birth` lane: it is a different defect from the one
that lane exists for (D112, birth-versus-write ordering), and fixing it there
would have made the D112 fix unprovable on its own.

Covering test, already in the tree and PARKED as red-first evidence:
`a_second_keystroke_into_a_fresh_slot_appends_rather_than_prepends`
(`frontends/gpui/tests/quick_open_returns_focus_windowed.rs`), carrying
`#[ignore]` that names this entry. Removing that `#[ignore]` is the first step
of the queued `slot-caret-advance` lane.
