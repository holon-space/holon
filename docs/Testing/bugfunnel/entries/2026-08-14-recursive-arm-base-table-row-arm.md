---
id: 2026-08-14-recursive-arm-base-table-row-arm
date: 2026-08-14
gap: COVERAGE
secondary: ENVIRONMENT
status: UNCLASSIFIED
summary: >-
  A recursive arm that `LEFT JOIN`s on the base-table row that arm itself
  produces never terminates.
source_line: 713
---

## Bug

(task-#12 turso-triage lane; found by the fuzzer lane, re-measured here
against both engines) **A recursive arm that `LEFT JOIN`s on the base-table
row that arm itself produces never terminates.** Bisected at fork head:
hangs with no `IS NULL` predicate at all, `INNER JOIN` in the same position
is fine, `LEFT JOIN` on the CTE row is fine, driving order irrelevant — so
the trigger is the `LEFT JOIN` target, not the anti-join — this refined rule
supersedes the fuzzer lane's broader bisect, which would condemn two
provably-safe production CTEs. Evidence `lane-logs/baudit-probes/t12_*.sql`
(probes E–I plus the three verbatim production replays, each named for its
verdict). Fork-head-only regression: correct at our pin.
`turso_seams.rs:200` (`get_blocks` walk) and `:249` (doc shape gate) match
the shape and HANG verbatim at fork head.

## Root cause

task-#12 turso-triage lane, found by the fuzzer lane and then re-measured
here against BOTH engines — no test produced it: **a recursive arm that
`LEFT JOIN`s on the base-table row that arm itself produces NEVER
TERMINATES.** Bisected at fork head with a 30s timeout: it hangs with NO `IS
NULL` predicate anywhere (probe G), an `INNER JOIN` in the same position is
fine (probe H), a `LEFT JOIN` whose `ON` references the CTE row is fine with
or without the anti-join conjunct (probes E, F), and the arm's driving table
is irrelevant (probe I hangs a CTE-driven arm; making the hanging arm
CTE-driven does not save it). So the trigger is the `LEFT JOIN` target, NOT
the anti-join, which is how the original "anti-join hang" framing was
stated. **This is a fork-head-only regression**: `fuzzlane_A` returns
correct rows at our pin `54f3cc5`. THEREFORE NO PRODUCTION EXPOSURE TODAY —
but `crates/holon-app/src/turso_seams.rs:200` (the `get_blocks` membership
walk) and `:249` (the doc shape gate) both `LEFT JOIN block_tags ON
bt.block_id = b.id` where `b` is the recursive arm's base table, and BOTH
HANG when run verbatim at fork head. `block_domain.rs:567` and the worker
seed `LEFT JOIN` on the CTE row instead and are safe at both. The re-pin
(task #22) would therefore freeze a production hot path; it needs those two
rewritten first. COVERAGE because the fuzzer could not generate the shape;
the gap-closing rung is the fork's statement timeout plus the armed shape
(task #23) — without the timeout a hang is indistinguishable from a slow
case. Secondary ENVIRONMENT: no Holon gate runs production SQL against a
re-pin candidate, so an engine-version-dependent defect can only be caught
by hand. THIS SUPERSEDES the fuzzer lane's broader bisect
("base-table-driven arm + CTE inner-join on equality + LEFT JOIN to a
different table + rows flowing"), which is wider than what actually fires
and would condemn two production CTEs that are provably safe. Repros
`lane-logs/baudit-probes/fuzzlane_A_recursive_arm_antijoin_HANGS.sql`,
`fuzzlane_D_generated_shape_replay_HANGS.sql`; the bisect probes E–I and the
three verbatim production replays are `lane-logs/baudit-probes/t12_*.sql`,
each named for its verdict (`..._HANGS` / `..._ok`), run with a 30s timeout
against `tursodb` built at both revisions. The checker exists as
`a_recursive_arm_never_left_joins_on_its_own_base_row` in
`crates/holon-turso/tests/recursive_cte_shape_architecture.rs`, `#[ignore]`d
because it is RED on those two sites today; it names exactly them and
nothing else, and gets un-ignored with the re-pin.)

## Missing piece

The fuzzer could not generate the shape, and without a statement timeout a
hang is indistinguishable from a slow case; separately, no Holon gate runs
production SQL against a re-pin candidate, so an engine-version-dependent
defect is only reachable by hand.

## Remedy

NO EXPOSURE AT OUR PIN. **Blocks task #22**: the re-pin would freeze the
`get_blocks` hot path until those two sites `LEFT JOIN` on the CTE row
instead. Gap-closing rung = the fork's statement timeout plus the armed
shape (task #23). Checker written as
`a_recursive_arm_never_left_joins_on_its_own_base_row`, `#[ignore]`d because
it is red on exactly those two sites today; un-ignored with the re-pin.
