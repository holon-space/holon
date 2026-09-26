---
id: 2026-09-10-second-keystroke-into-a-fresh-slot-inserts-at-caret-zero
date: 2026-09-10
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
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

FIXED. `birth_creation_affordance` (`crates/holon-frontend/src/reactive.rs`)
now seats focus on the newborn WITHOUT arming a caret offset. The newborn's
editor mounts after the edit that births it has landed, so the mount's default,
end-of-text, puts the caret after the typed text. The fix is in the shared
reactive layer, so every frontend gets it.

Covering tests, red on the pre-fix birth and green after:
- windowed: `a_second_keystroke_into_a_fresh_slot_appends_rather_than_prepends`
  (`frontends/gpui/tests/quick_open_returns_focus_windowed.rs`), no longer
  ignored; red `"ba"`.
- headless keystone: hand-authored case
  `typing-several-characters-into-a-creation-slot-keeps-their-order`
  (`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`);
  red `milka` for typed `a milk`. The headless rung DOES reproduce it: its
  editor mirror adopts the armed seed on the newborn's first keystroke exactly
  as the GPUI mount does, so the ENVIRONMENT classification above was too
  pessimistic — the real gap was COVERAGE (no case typed two characters into a
  slot, and the generator rarely stayed on a seated slot long enough to type).
- generator: `TypeChars` weight is raised while the caret sits on a creation
  slot (`crates/holon-integration-tests/src/pbt/transitions/type_chars.rs`).
