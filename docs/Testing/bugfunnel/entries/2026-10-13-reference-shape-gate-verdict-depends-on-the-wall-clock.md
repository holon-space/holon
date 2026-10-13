---
id: 2026-10-13-reference-shape-gate-verdict-depends-on-the-wall-clock
date: 2026-10-13
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The reference model's shape-gate verdict depended on whether a millisecond
  ticked, so the same seed stepped the reference differently from run to run
  and the SqlOnly typing coverage floor went red in about 1 run of 3.
---

## Bug

`pbt::composed::wide_e2e::tests::sqlonly_generated_cases_commit_typed_edits_into_block_content`
failed about 1 run in 3 with `SqlOnly TYPING coverage floor: only 1/64`. The
binary and the seed were the same; the typing share was 1, 2, 2 or 4 of 64.
Found as a flaky red by lane holon-space/holon-work#205 (escape class: model
gap — the reference was not deterministic).

## Root cause

`broken_shape` (`crates/holon-integration-tests/src/pbt/shape_state.rs`)
decides whether a step changed a decision subtree by comparing whole `Block`
values. It normalizes sequence values with `set_sequence`, and `set_sequence`
writes `updated_at = now_millis()` (`crates/holon-api/src/block.rs`). The
before- and after-subtrees are built at different instants. When a millisecond
ticked between them, the subtree of an already-broken decision (`block:kd-1`,
rule `DC1`) read as changed, and the model refused an unrelated step: for
example `SplitBlock(split-1, 0)` on `structural-page`, or an `Indent`. The
refused step left the model on a different path, which changed what the
generator drew next, and with it how many cases typed.

A lockstep probe stepped two references from the same deterministic runner.
The first divergence was always a step that one run refused with
`step_refusal: DC1` and the other run applied. The subtree dumps differed only
in `updated_at` (`...127781` vs `...127780`). With the fix, the probe stepped
64 cases x 40 steps with no divergence.

## Missing piece

Nothing checks that the reference model is deterministic: two references stepped
from the same seed are never compared. A wall-clock dependence in any reference
decision shows up only as a flaky statistic far away from its cause.

## Remedy

`broken_shape` clears `created_at` and `updated_at` after it normalizes the
sequence, so its verdict depends only on what the decision adapter reads. The
temporary known-red row `sqlonly-typing-floor-flake` is removed. Generalizable:
yes. Every reference that stamps `Block::new` or `set_sequence` times into
state that a decision reads can have the same defect. A same-seed lockstep
check of two references would catch the class.
