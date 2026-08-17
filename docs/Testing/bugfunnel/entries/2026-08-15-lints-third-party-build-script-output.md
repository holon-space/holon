---
id: 2026-08-15-lints-third-party-build-script-output
date: 2026-08-15
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  `archlint --all` lints third-party build-script output as our source, so the
  architecture gate's verdict depends on which OTHER gate ran first in the
  same checkout.
source_line: 704
---

## Bug

(task-#29 gate-composition lane; found by MEASURING the architecture gate
twice in one tree while composing it into the landing gate; no test produced
it) **`archlint --all` lints third-party build-script output as our source,
so the architecture gate's verdict depends on which OTHER gate ran first in
the same checkout.** `collect_all_files` (`archlint/archlint.py:646`) globs
`frontends/**/*.rs` with no `target/` exclusion; `frontends/holon-worker` is
out-of-workspace and owns a NESTED `target/`, so `just check-worker-wasm`
(Tier-1 `precommit` step 3/3) writes generated `.rs` into the globbed tree.
A/B, same source: PASS 5/5 → `check-worker-wasm` → FAIL with 4 violations,
all under `frontends/holon-worker/target/`. The two gates are mutually
exclusive, and `--update-baseline` in that tree state would ratchet vendored
code into `archlint/baseline.txt`.

## Root cause

secondary ENVIRONMENT, task-#29 gate-composition lane, found by MEASURING a
gate twice in one tree while composing it into the landing gate — no test
produced it, and the run that would expose it is the one nobody makes:
**`archlint --all` lints third-party build-script OUTPUT as if it were our
source, so the architecture gate's verdict depends on which OTHER gate ran
first in the same checkout.** `collect_all_files`
(`archlint/archlint.py:646`) globs `frontends/**/*.rs` with no `target/`
exclusion. The repo's own `target/` sits at the root and is never globbed,
which is why this never bit — but `frontends/holon-worker` is an
out-of-workspace crate with its OWN nested `target/`, so `just
check-worker-wasm` — Tier-1 `precommit` step 3/3 — drops generated `.rs`
straight into the globbed tree. A/B in one workspace, same source: `cargo
nextest run -p holon-architecture-tests` PASSED 5/5 at 22:36;
`check-worker-wasm` then wrote `clang-sys/out/common.rs` (23:22) and
`jetscii/out/src/macros.rs` (23:26); the SAME command FAILED at 00:2x with 4
violations (`ok`, `filter_map_ok`, `fallback` ×2) whose every path was under
`frontends/holon-worker/target/`. So the two gates are mutually exclusive:
run precommit, and the architecture gate is red on a clean tree. It also
runs the baseline ratchet on foreign code — the same run printed `baseline
stale - 4 entry(ies) no longer fire`, i.e. an `--update-baseline` in the
wrong tree state would have written vendored build output into
`archlint/baseline.txt`. CI never saw it (fresh clone, one gate per job).
Primary ORACLE — the check ran and returned a WRONG verdict, judging files
that are not the system under test; secondary ENVIRONMENT — the
discriminator is tree state that differs between CI and any local tree that
ran a sibling gate. FIXED: `collect_all_files` now drops any path with a
`target` component. Pinned by A/B re-measurement, not a new test: with the
offending artifacts still on disk the gate went 4-violations-red → 5/5 green
across only that edit (`lane-logs/measure-gate-arch-warm.log` vs
`lane-logs/measure-gate-arch-warm2.log`). **SECOND SITE, same defect, found
2026-08-15 while running Tier-1 `precommit` end-to-end for task #31:**
`scripts/check-defensive-code.sh` greps `crates/ frontends/` recursively
with no `target/` exclusion either, so the defensive-code ratchet —
`precommit` step 1/3 — was judging the same vendored build output (3 of 828
scanned lines). Fixed by adding `/target/` to its `TEST_EXCLUDE`; 828 → 825
with 0 worker-target entries. Two independent scanners had the identical
hole, so treat "does this scanner exclude build output?" as a checklist item
for any new tree-walking gate rather than a one-off.)

## Missing piece

the check ran and returned a wrong verdict because its file set is unbounded
— nothing constrains archlint's scan to the system under test, and no test
runs it twice in different tree states

## Remedy

FIXED 2026-08-15: `collect_all_files` drops any path with a `target`
component. Proven by A/B re-measurement with the artifacts still on disk —
4-red → 5/5 green across only that edit
(`lane-logs/measure-gate-arch-warm.log` vs `warm2.log`).
