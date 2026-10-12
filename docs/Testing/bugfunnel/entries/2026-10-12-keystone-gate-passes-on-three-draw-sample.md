---
id: 2026-10-12-keystone-gate-passes-on-three-draw-sample
date: 2026-10-12
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A registered known red ended the keystone run on its first hit, so a
  PROPTEST_CASES=16 sweep completed three draws and the known-reds gate still
  reported PASS-WITH-NOTE over that almost empty sample.
---

## Bug

`just keystone-full` on main 53c5b430 (`PROPTEST_CASES=16`) completed **3
draws**. Draw 4 tripped the registered known red `syn-real-mint`
(crates/holon-integration-tests/src/pbt/composed/harness.rs:1874); proptest
shrank it 18 times and the run ended. The remaining 12 configured draws were
never made, and `scripts/keystone-known-reds.sh` classified the log as
`PASS-WITH-NOTE` with exit 0 — the whole invariant catalog had been exercised on
a fifth of the sample the operator asked for, and the gate said pass.

Found by the steward reading the run log, not by any test.
Log: `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/e00a77ad-4a49-4d9a-926c-df8968cbe36d/scratchpad/ksf30/ksf.log`
(`successes: 3`, `test result: FAILED`, 18 × `harness.rs:1874`).

## Root cause

Two independent holes, one on each side of the gate:

- The keystone test body
  (crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs) let every
  panic become a proptest `TestCaseError::Fail`. proptest's contract is to stop
  at the first failing case and spend `max_shrink_iters` on it, so the FIRST
  draw hitting an already-registered red terminated the sweep. The registry
  exists precisely to say "this one is known" — but nothing read it at run time,
  only afterwards.
- The classifier judged *signatures* and never the *sample*. Its one nod to
  sample size was `WARN: low-power run` on proptest's `successes:` count against
  a hardcoded 32 — a warning, not a verdict, and a constant unrelated to the
  configured case count.

## Missing piece

Nothing in the harness or the gate asserted **how many draws the verdict rests
on**. A known red was allowed to cost the run its whole remaining sample, and
"all signatures are registered" was treated as sufficient for a pass.

## Remedy

- `crates/holon-integration-tests/src/pbt/known_reds.rs` (new) parses the same
  `| key | status | pattern |` columns of docs/Testing/KeystoneKnownReds.md the
  classifier does, and matches the same normalized first message line. The
  registry stays the single source of truth.
- ONE catch boundary in the keystone test body: a draw whose panic matches a
  `known-red` row is **recorded and spent** — the panic stays on stderr for the
  log and the classifier, and the run draws the next case. A novel signature
  `resume_unwind`s, so it fails the run with its shrink exactly as before.
- Every run discloses `[keystone-sample] cases=… attempted=… completed=…
  floor=… known-red-hits=… verdict=OK|FLOOR-BREACH`, plus one line per
  signature.
- The floor is **half the configured cases, at least one**
  (`known_reds::completed_floor`, the ONE place it lives). Below it the run
  panics and the classifier reports FAIL. A keystone log with no
  `[keystone-sample]` line at all is also a FAIL: it stopped at its first
  failure or was truncated, so its sample is unverified. The rotting
  `successes: … < 32` warning is gone.

Evidence: the same ksf.log classifies PASS-WITH-NOTE / exit 0 on the old tree
and FAIL / exit 1 after
(`target/ks-draws-before-classifier.txt`, `target/ks-draws-after-classifier.txt`).
