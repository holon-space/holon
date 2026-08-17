---
id: 2026-08-16-mounted-gate-branch-dead-real-node
date: 2026-08-16
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  `ReactiveEngineDriver::set_block_expanded`'s mounted-gate branch was DEAD
  for every real node: it stripped the `block:` scheme (`user_driver.rs:905`,
  `let bare = …strip_prefix("block:")`) before `find_expand_toggle`, but the
  `expand_toggle` shadow builder stores `target_id` as the row's SCHEMED `id`
  (`shadow_builders/expand_toggle.rs:17`, `ba.ctx.row().get("id")`), and the
  lookup is a plain string equality — so it never matched and every drive fell
  silently through to the embedded-page branch, which writes NO document
  field.
source_line: 1203
---

## Bug

(D25.a lane-driver-fold, found by a fresh-context VERIFIER probing the
branch this lane rewrote — no test verdict; corroborated in-lane by
measurement) **`ReactiveEngineDriver::set_block_expanded`'s mounted-gate
branch was DEAD for every real node: it stripped the `block:` scheme
(`user_driver.rs:905`, `let bare = …strip_prefix("block:")`) before
`find_expand_toggle`, but the `expand_toggle` shadow builder stores
`target_id` as the row's SCHEMED `id`
(`shadow_builders/expand_toggle.rs:17`, `ba.ctx.row().get("id")`), and the
lookup is a plain string equality — so it never matched and every drive fell
silently through to the embedded-page branch, which writes NO document
field.** The strip and the "a bare block id, no `block:` scheme" doc on the
walk both predate this lane. Nothing caught it because the view-local
expansion STORE normalizes its key on read and write
(`reactive.rs:1977-2001`, `strip_prefix("block:")` on both legs), so the
store leg worked under either spelling and only the node lookup ever saw the
difference — the normalization comment even NAMED the divergence ("the
driver strips the `block:` scheme while the builder reads the (schemed) row
`id`") and papered over it instead of fixing it. Mutation corroboration from
the verifier: no-opping the whole mounted branch leaves every suite green.
MEASURED IN-LANE and load-bearing for whoever picks this up: fixing the
scheme is NECESSARY BUT NOT SUFFICIENT for a headless proof — enumerating
every node of `snapshot_reactive(root_layout)` (children + slots +
materialised lazy caches) finds **ZERO `expand_toggle` nodes under any
spelling** both in `setup_embedded_page_sut` and in a purpose-built mounted
fixture, so the branch cannot be reached headlessly at all. The
embedded-page topology has none BY DESIGN (its toggle is synthesized during
recursive resolve — the very reason the driver's second branch exists), and
a bare org fixture renders an EMPTY root layout for want of root-layout
scaffolding. SCOPE, adjudicated with the verifier and load-bearing for how
alarming this row should read: the branch is **HARNESS-ONLY and NO
PRODUCTION FRONTEND WAS EVER AFFECTED** — `GpuiUserDriver` synthesizes a
chevron click (`frontends/gpui/src/user_driver.rs:767`) and never calls
`find_expand_toggle`, and the worker writes only the expansion store. This
is a harness-correctness defect: the harness silently stopped modelling the
affordance it claims to drive, which is exactly what D25.a exists to fix.

## Missing piece

the failing code path does not run in ANY headless wiring — the reactive
snapshot the driver walks contains no `expand_toggle` node in either
available topology, so no headless invariant could have gone red however
strong; secondary COVERAGE because the keystone additionally never GENERATES
`ExpandToggle`/`CollapseToggle` at all (0 draws in 32 cases on both the lane
tree and untouched main — task #23), so even a reachable branch would sit
untouched

## Remedy

FIXED (the mismatch) 2026-08-16 in this lane: the driver passes the schemed
id throughout, the false "bare block id" doc on `find_expand_toggle` is
corrected to state SCHEMED, and the store's normalization comment no longer
describes a stripping driver. PINNED at the unit level by
`find_expand_toggle_requires_the_schemed_id` (schemed matches, stripped does
NOT) plus a fixture that now DERIVES `target_id` from the row id exactly as
the builder does, so it can no longer express a node the builder cannot
produce. NOT PINNED, stated rather than faked: a re-introduced strip **in
the driver** still cannot go red anywhere, because no headless topology
executes that branch — the end-to-end guard
`mounted_expand_toggle_dispatches_the_collapsed_write` is committed
`#[ignore]`d with its measurement in its doc comment. Un-ignoring it needs a
harness fixture that MOUNTS an `expand_toggle` in the root layout; it is NOT
a windowed port, since no production frontend calls this branch either.
RECONCILIATION of the two measurements, which look contradictory and are
not: the verifier's probe proved the MATCHING RULE (schemed matches,
stripped does not) on a synthetic `interpret_pure` node, while this lane's
enumeration proved that no headless root-layout MOUNTS such a node. Both
hold — they are statements about different objects, and only together do
they explain why the strip was both genuinely wrong and completely
invisible. Red-first for the fix itself:
`lane-logs/driver-fold-scheme-RED.log` (`collapsed=0` — driver never
dispatched).
