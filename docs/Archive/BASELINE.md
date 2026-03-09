# PBT Baseline (ADR 0004–0007 migration — Phase −1)

Pinned reference for the blessed PBT suites so later phase gates mean **"no
*new* failure mode"**, not "zero failures". A suite that is flaky or failing
*here* does not block a phase — only a *new* failure introduced by a change does.

**Status:** retroactive. Assembled 2026-05-29 from this session's runs plus
project history, **not** from a fresh N≥5 sweep (a full sweep costs hours — see
"How to refresh" below). Treat the entries below as confirmed-observed, and
re-measure properly before relying on the gate for a high-stakes phase.

Pinned base commit for "fails identically pre-change" checks: `70012b41` (the
commit Phase 8 was built on).

## Blessed suites

| Suite | Test | Status at baseline |
|-------|------|--------------------|
| general_e2e_pbt (Full / Loro) | `general_e2e_pbt` | **GREEN** — PASS 550s with `PROPTEST_CASES=1` replaying all persisted Loro regression seeds (2026-05-29, post sort_key removal + fixed-point fix). |
| general_e2e_pbt (SqlOnly) | `general_e2e_pbt_sql_only` | **RED (pre-existing framework artifact)** — see KF-1. |
| loro_backend_pbt | `api::loro_backend_pbt::stateful_tests::test_loro_backend_state_machine` | **RED (pre-existing harness artifact)** — see KF-2. |
| org_create_ordering_pbt | `org_create_ordering_pbt_full` | GREEN in targeted runs; intermittently trips cluster #6 — see KF-3. Memory: "NOT a reliably-green gate." |

## Known-failing / known-flaky (do NOT attribute to a new change)

- **KF-1 — `general_e2e_pbt_sql_only` `seen_transitions_counter` panic.**
  `proptest-state-machine` (strategy.rs:590) panics "Unexpected non-zero
  `seen_transitions_counter`" when replaying stale persisted regression seeds
  whose recorded transition count no longer matches the current alphabet. It is
  a *framework replay artifact*, not a Holon invariant failure. Fires before any
  invariant runs.

- **KF-8 — `general_e2e_pbt_sql_only` intermittent Strict invariant panics under
  stale-seed replay (`inv-focus-roots`, `inv-blocks-match-ref/matview`).** Beyond
  the deterministic KF-1, replaying the persisted regression seeds surfaces a
  *nondeterministic* Strict invariant failure that varies run-to-run: observed
  `inv-focus-roots` (region `main` expected `{}` vs SUT `{block:journals}` — the
  error body self-identifies as a close-path/reference-model race, "NOT a Turso
  IVM drift") on one run and `inv-blocks-match-ref/matview` on the next, and
  neither on a third (base `69042497`). The CDC-lag / render-lag timing families
  (cf. KF-4/KF-5). Confirmed independent of the Phase-2a `ReferenceDomainState`
  re-home: `expected_focus_root_ids` reads only `open_pins` (not a moved field),
  and the re-home is value-preserving. Verified 2026-05-29 across base + two
  post-change runs (`PROPTEST_CASES=1`).

- **KF-2 — `test_loro_backend_state_machine` "Block ID mapping should be
  consistent".** Triggered by duplicate block content (`"x","x"`); a
  test-harness matching artifact. Confirmed failing **identically on base
  `70012b41`** via isolated worktree → not a regression.

- **KF-3 — `org_create_ordering_pbt` cluster #6.** Render + source synthetic-child
  ordering edge case; intermittent. Reproduces with shadow/feature changes
  disabled → pre-existing.

- **KF-4 — `inv-matview-consistent-with-ref` WARN spam (Full).** Fires many times
  as a non-fatal WARN; a known Turso IVM matview-drift behavior, not Strict-fatal.

- **KF-5 — CDC-quiescence `[("all_blocks", 1)]` post-StartApp.** Intermittent
  settling tail in churn-heavy sequences; a harness race, historically benign.

- **KF-6 — `render_eval::tests::test_state_display` (holon-api).** Unit test whose
  expectation disagrees with its own function body (`"TODO" => ("TODO","muted")`);
  `render_eval.rs` is untouched by the migration. Pre-existing.

- **KF-7 — `holon-org-format models::tests::test_block_to_org`.** Asserts a headline
  renders as `** TODO [#A] Test headline :work:urgent:`; current `Block::to_org`
  output disagrees. `holon-org-format/src/models.rs` (home of `to_org`) was NOT
  modified by Phase 8, the Phase-8 debt commit, or the #5 grouping work (verified
  via `jj diff --name-only`), so this predates the whole effort. Same class as KF-6.

- **KF-8 — `general_e2e_pbt_sql_only` `inv-blocks-match-ref/matview` reparent
  divergence (deep churn).** At `PROPTEST_CASES≥8` a fresh block created/moved
  under a nested parent gets the reference parent but the SQL write keeps the
  document-root parent (e.g. ref `block:X` under `block:r0--y…`, matview under
  `block:ref-doc-0`). Confirmed **pre-existing**: reproduces identically on base
  `70012b41` (`cargo test`, cases=12, 295 s) — NOT a Phase-8 regression, NOT the
  de-Loro rename. A SqlOnly reparent/move-projection write-side gap; `/block_raw`
  excludes `parent_id` so only `/matview` catches it. Detail:
  `devlog/2026-05-29-sql_only-matview-reparent-divergence.md`. Only surfaces at
  the suite's default `cases=8`, not at the `PROPTEST_CASES=1` baseline.

- **KF-9 — `general_e2e_pbt` (Full) borderline on the 600 s cap at
  `PROPTEST_CASES=1`.** N=5 sweep: runs 1 & 5 hit **TIMEOUT at exactly 600 s**;
  runs 2–4 PASSED (~550 s). The failures are NOT invariant divergences — Full at
  cases=1 runs right at the `.config/nextest.toml` 600 s hard cap and
  intermittently crosses it. So "Full green" depends on shaving ~50 s of margin.
  Mitigate: raise Full's nextest cap, or run Full via `cargo test` (no per-test
  cap) for gating. Not a correctness flake.

- **Sweep-contamination note (2026-05-29 N=5 run).** The N=5 sweep was run from
  the shared working copy; partway through (≈20:48, during the `sql_only`→
  `org_create` boundary) the working copy stopped compiling due to a parallel
  session's in-flight `reference_state` `.ui` extraction. Therefore
  `org_create_ordering_pbt_full` (5/5 `rc=101`, ~10 s each) and `sql_only` run 5
  in that sweep are **compile failures, not test results** — DISREGARD them.
  Valid data points from that sweep: Full (KF-9, ran 19:40–20:26), `sql_only`
  runs 1–4 (KF-1, ran before the break), `loro_backend` run 1 (KF-2). Re-run
  `org_create` from a compiling tree to get its real baseline.

## Measurement caveat (sweep config)

The suite's own default is `cases: 8` (`pbt_config()`), but `.config/nextest.toml`
hard-caps these PBT tests at **600 s** — Full at `cases=8` exceeds that, so a
`cases=8` run under nextest is killed (TIMEOUT) deterministically. The green
figures here were measured at `PROPTEST_CASES=1` (the only config that completes
under the cap under nextest). For heavier case counts use plain `cargo test` (no
per-test cap) — that is how KF-8 was reproduced. The `cases:8` guidance below
therefore applies only under `cargo test`, not `cargo nextest`.

## How to refresh (proper Phase −1 sweep)

Run each blessed suite N≥5× and record flaky seeds + invariants. Fast sanity loop
(replays persisted seeds + 1 random case, no re-shrink):

```
PROPTEST_CASES=1 PROPTEST_MAX_SHRINK_ITERS=0 \
  cargo nextest run -p holon-integration-tests --test general_e2e_pbt 2>&1 | tee /tmp/baseline-run.txt
```

For a real flakiness measure use the default `cases: 8` and repeat ≥5 runs per
suite, logging any seed that flips pass/fail.
