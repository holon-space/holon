---
id: 2026-09-30-dense-patch-create-of-a-task-headline-refused
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An MCP `dense_patch` that adds a new headline carrying a task keyword
  (`** TODO x`, `** ? Which store?`) is refused at its create op, because the
  patch sends `task_state_category` as a create param and the engine's drawer
  check refuses that key as a typed field.
---

## Bug
Found by the decision Inc 7 round-3 verifier (`lane-logs/inc7r3v-verify.md`,
DEFECT 1) by forcing the `#[ignore]`d test
`tools::dense_patch_shape_gate_tests::a_legal_decision_is_created_by_one_dense_patch`.
Its first failure was the test's own seed: `root` sat under
`sentinel:no_parent`, and a dense projection renders under its roots' parent,
which the org renderer refuses for the sentinel. With the seed fixed (round 4,
`lane-logs/inc7r4-d1.log`) the test reaches the patch and fails at op 1:

`create: refusing property key "task_state_category" of block block:… via
`task_state_category`: the org parser reads that drawer key back as the
block's typed field (an edge or a storage column), not as a property`.

## Root cause
`patch_op_calls` (`frontends/mcp/src/tools.rs`, `PatchOp::Create`) pushes
`task_state` and `task_state_category` as create params when the headline has
a keyword. The engine's drawer-key check
(`OperationEngine::reads_back_as_a_typed_field`,
`crates/holon/src/api/operation_engine.rs:1993`) maps
`task_state_category` to `TypedDrawerKey::TaskState`
(`crates/holon-org-format/src/drawer.rs:153`) and refuses it. The check came
with the org-faithful group B commit `0e83521bc1c4`; the create path of
`dense_patch` was not adjusted. Every dense_patch that creates a keyword
headline is refused and rolled back.

## Missing piece
The keystone's `DenseProjectionEdit` (`AppendChild`) appends only
keyword-less titles
(`crates/holon-integration-tests/src/pbt/transitions/dense_projection_edit.rs:72`),
so no generated patch creates a task headline.

## Remedy
Fixed in two parts. Decision Inc 6 (on main) sends the keyword through the
create op's `task_state` alone; the engine derives the category from the
document's `#+TODO:` ring. Decision Inc 7 round 6 judges a plan in the form
the run dispatches it: the engine classifies each op's task state before the
shape gate judges the plan (`DispatchingOperationEngine::judged_run`), so a
create with a keyword matches its judged op and is not judged again alone
(before: refused with DC1, `lane-logs/inc7r6-mcp-1.log` in the Inc 7
workspace). Pinned by
`tools::dense_patch_shape_gate_tests::a_legal_decision_is_created_by_one_dense_patch`.
Still open: the keystone's `AppendChild` draws no keyword headline.
