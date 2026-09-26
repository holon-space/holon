---
id: 2026-09-27-dense-patch-drops-a-drawer-reorder
date: 2026-09-27
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A dense_patch that reordered a row's drawer lines planned nothing, and a
  reorder plus a value change applied only the value (a partial apply); a new
  row's drawer came back in key order, not the order the agent wrote.
---

## Bug
Found by an adversarial verifier agent driving `plan_patch` over
`build_projection` (lane report `lane-logs/inc6r2v-verify.md`, D-R2-1). The
edit `:zeta: 1` / `:alpha: 2` → `:alpha: 2` / `:zeta: 1` planned 0 ops, yet
the authored order is stored state (`_drawer_order`) that the org renderer
replays, so the file kept the old order and the patch reported success.

## Root cause
The planner compared drawer lines as a key/value map; nothing wrote the
authored drawer order.

## Missing piece
The exactness oracle sorted the drawer lines before comparing, no generated
edit moved a line, and no generated store held an authored order.

## Remedy
The planner writes `_drawer_order` (`PatchOp::SetDrawerOrder`, and on a
create) whenever the renderer's own order rule (`drawer_key_order`) would not
reproduce the edited line order. The oracle in
`frontends/mcp/tests/dense_patch_exact.rs` compares the ordered lines and the
rendered row text; generated edits move lines and generated stores hold an
authored order. Harness rung 7 reads the order back from the store and the
file. Red on the old planner: `lane-logs/inc6r3-red.log`.
