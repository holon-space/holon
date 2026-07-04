# Doc-block count divergence (keystone `inv-blocks-match-ref/matview`)

Date: 2026-07-07
Worktree: `.claude/worktrees/advice-spikes`

## Repro

```
PROPTEST_CASES=6 \
HOLON_PBT_WEIGHTS='WriteOrgFile:100,SetEdgeField:100' \
HOLON_PBT_INVARIANTS='inv-org-render-fixed-point:skip' \
cargo test -p holon-integration-tests --test general_e2e_composed_pbt --features pbt
```

Deterministic red on `inv-blocks-match-ref/matview`: SUT block matview = 34
blocks, reference = 33. Full log:
`/Users/martin/.claude/jobs/3c2ea51f/tmp/docblock-count.log`
(panic at `crates/holon-integration-tests/src/pbt/composed/harness.rs:463`).

## The divergence (precisely)

The parent framing ("extra distinct `ref-doc-6` doc page") is **not** what
happens. The id `block:ref-doc-6` appears **twice** in the SUT matview and
**once** in the reference. It is a **duplicate row**, not a distinct extra
block. The two SUT copies are the *document* block (`parent = block:__document_root__`,
`content = "__sdue_z_i__z__5325478578703"`) and differ in exactly one field:

- copy A: `tags: {"Page"}`
- copy B: `tags: {}`   ← the surplus row

Everything else (id, parent, content, requires, properties) is identical. The
reference holds only copy A.

Differing only by the join-produced `tags` facet is the signature of a
**LEFT-JOIN cross-product fan-out over the junction tables**, not a base-table
double-insert (which would yield two byte-identical rows). The SUT `block_raw`
base is fine; the duplication is in the `block` **matview**.

Trigger: under the forced alphabet the doc page `ref-doc-6` (minted by
WriteOrgFile, tagged `Page` at ingest) also receives `SetEdgeField` edge-field
writes. The SQL log confirms tag/requires writes land on it:

```
INSERT INTO block_tags   ("block_id","tag") VALUES ('block:ref-doc-6','Page')
DELETE FROM block_tags   WHERE "block_id" = 'block:ref-doc-6'
DELETE FROM block_requires WHERE "block_id" = 'block:ref-doc-6'
```

The matview extraction (`extract_matview` →
`sut.live_block_snapshot()` in
`crates/holon-integration-tests/src/pbt/composed/correspondences.rs:131`) reads
the matview faithfully and does **not** dedup, so a genuine duplicate matview
row surfaces as two `Block`s.

## Root-cause verdict: PROD (Turso IVM matview), NOT harness, NOT advice

This is the already-known **"duplicate row in the `block` matview"** bug,
captured verbatim in
`crates/holon-integration-tests/tests/matview_duplicate_row_repro.rs`
("on edge-field `tags` then `requires` writes to one block, the `block`
matview ends up with two rows for that block; base state is correct").

Mechanism / owner:
- `block` matview synthesized by `BlockMatviewSchemaModule`,
  `crates/holon-turso/src/schema_modules.rs:256` — SELECT built by
  `block_matview_select` (`schema_modules.rs:230`): `block_raw` LEFT-JOINed
  against one per-junction aggregation matview each
  (`edge_agg_view_select`, `schema_modules.rs:210`).
- The chained-matview refactor (one agg matview per junction, at most one row
  each — `schema_modules.rs:219-246`) was the intended fix for the plain-SQL
  fan-out and is pinned by
  `holon-advice/tests/matview_build.rs::probe_multi_junction_fanout_fix_shapes`.
- The composed keystone still reproduces a duplicate for this shape (doc-page
  block carrying a `Page` tag + a requires edit), so the residual is an
  **incremental-maintenance (IVM) correctness gap in Turso**: after the
  tag/requires edits the LEFT-JOIN (or the per-junction agg matview) yields a
  stale row (`tags = []`) alongside the updated row (`tags = [Page]`) instead
  of collapsing to one. Turso IVM is the user's own fork (see MEMORY:
  `turso_ivm_is_ours_extendable`), so the fix belongs there / in the matview
  synthesis, not in the harness.

Not advice-specific, not the requires-roundtrip fix, and **not** a WriteOrgFile
reference-accounting gap — the reference is correct.

## Fix applied? NO.

Per the time-box: the fix touches prod Turso IVM / matview synthesis and the
mechanism (why the incremental LEFT-JOIN emits a stale + updated row) is not a
one-line, unambiguously-correct change. Any harness-side dedup would **mask** a
real prod duplicate-row bug (forbidden). Left red intentionally.

## UPDATE 2026-07-07 (session 2): reproduced Turso-side, root-cause localized, NOT fixed

### What was proven
- The keystone still reproduces deterministically (34 vs 33; duplicate row is
  `block:ref-doc-6`, `tags={Page}` beside `tags={}`).
- Captured holon's exact SQL statement stream (temporary `HOLON_CAPTURE_SQL`
  eprintln hook in `holon-turso/src/turso.rs` `trace_sql*`, since reverted).
  The full stream (2438 base-table DML statements, 221 blocks) is saved as
  `turso/tests/integration/query_processing/ref_doc6_replay.tsv`.
- **Turso-side reproduction achieved** by verbatim replay of that stream against
  the exact `block` matview chain (`block_raw LEFT JOIN block_requires_agg
  LEFT JOIN block_tags_agg LEFT JOIN advice_suppressed_agg`):
  `turso/tests/integration/query_processing/test_ivm_block_matview_replay.rs`.
  `REPLAY_MODE=auto`/`txn` both leave a duplicate `block` row (a stale
  null-padded LEFT-JOIN row beside the matched row) — the same signature as the
  keystone.

### Precisely what triggers it (from `REPLAY_MODE=trace`)
- The first duplicate-producing statement is always a **`block_raw` UPSERT**
  (`INSERT ... ON CONFLICT DO UPDATE`, i.e. a left-side δL update of content/
  updated_at) on a block that is already tag-**matched** (`tags=['Page']`).
- **Batching does NOT decide it.** Initially it looked like transaction batching
  (holon's sync path) was clean and per-statement autocommit (holon's ingest/
  scan path) was buggy — but running both modes in one process showed the
  duplicate appears in **either** mode. The deciding factor is **HashMap
  iteration order** (fixed per process): it determines whether the stale
  null-padded ghost survives. The victim block varies run-to-run
  (`block:__default__`, `block:c2`, `block:default-advice-rules`,
  `block:ref-doc-6`, …) → transient null-padded ghosts are **pervasive** during
  incremental maintenance of this chained shape; some snapshot always catches one.
- **Single-block replay is CLEAN** (`test_..._ref_doc6_only`, 68 statements) and
  every hand-built single/multi-block shape I tried is CLEAN
  (`test_ivm_agg_matview_stale_group_on_delete.rs`, all PASS). The bug requires
  the **cross-block interleaved deltas through the SHARED agg matviews** plus a
  δL update on a matched row.

### Root-cause localization (operator level, not yet pinned to a line)
- The `block` matview's three chained LEFT JOINs each compile to an
  Inner + Antijoin + Merge subgraph (`core/incremental/{join,antijoin,merge}_operator.rs`).
  `block_tags_agg` is the MIDDLE junction, so the tags-join's LEFT input is the
  MERGE output of the prior (requires) LEFT JOIN — the "chained LEFT JOIN fed by
  a merge" scenario the existing `JoinOperator::commit` consolidate fix targets.
- The residual: a δL update on a matched left row spuriously leaves (or fails to
  retract) a null-padded antijoin row for that key, order-dependently, when the
  shared agg matview has been churned by other blocks. `JoinOperator` and
  `AntijoinOperator` already `consolidate()` their inputs; `MergeOperator::commit`
  does NOT consolidate — a candidate, but unconfirmed. The exact leaking operator
  + state transition needs btree-state tracing across the interleaved passes.

### Why NOT fixed
- Serious effort spent; a clean, unambiguously-correct one-operator fix was not
  found. The obvious consolidate fix already exists in `JoinOperator::commit`.
  The residual is a subtle, cross-block, hash-order-dependent state bug — forcing
  a speculative change here risks masking or shifting it. Stopped per the
  time-box guardrail. No harness-side dedup added (would mask the real bug).

### Turso-side artifacts left in the working tree (branch `holon`, uncommitted)
- `test_ivm_block_matview_replay.rs` (+ `ref_doc6_replay.tsv`, `ref6_only.tsv`):
  the reproduction + acceptance gate. Two tests `#[ignore]`d (KNOWN-RED,
  seed-flaky); `test_..._ref_doc6_only` passes.
- `test_ivm_agg_matview_stale_group_on_delete.rs`: hand-built shapes that do NOT
  reproduce (negative results, all PASS) — documents the minimal shape is
  insufficient.
- No operator code changed. IVM `query_processing` suite: 94 passed, 4 ignored.

### Recommended next step (for the fix)
1. Use `REPLAY_MODE=auto` (or `trace`) on `test_ivm_block_matview_replay.rs` as
   the driver; it deterministically leaves a ghost (victim varies by seed).
2. Trace the antijoin `R_COUNT`/`L_INDEX` and merge state for the victim key
   across the δL-update pass that first creates the ghost (statement index from
   `REPLAY_MODE=trace`). Confirm whether the null-padded row is added by the
   antijoin R-term or not retracted by the inner join / merge.
3. Fix minimally in `core/incremental/`; the replay auto-mode test is the gate.

## UPDATE 2026-07-07 (session 3): MergeOperator-consolidate hypothesis FALSIFIED, revert clean

### Hypothesis tested (single, time-boxed)
Adding delta consolidation at `MergeOperator::commit` (mirroring the existing
`JoinOperator::commit` / `AntijoinOperator::commit` consolidate-at-commit fix)
eliminates the stale null-padded ghost row.

### Confirmed precondition
`MergeOperator::commit` (`core/incremental/merge_operator.rs:171`) indeed does
NOT consolidate: it calls `transform_delta` on left/right then `output.merge()`
(which explicitly does NOT consolidate — see `dbsp.rs:247` "preserves order, no
consolidation"). Join (`join_operator.rs:665-667`) and Antijoin
(`antijoin_operator.rs:671-672`) both `deltas.left/right.consolidate()` at the
top of `commit`. So the asymmetry the prior session named is real.

### Result: FALSIFIED
Applied the exact mirror (`deltas.left.consolidate(); deltas.right.consolidate();`
at the top of `MergeOperator::commit`, before `transform_delta`). Rebuilt, reran
the `auto` replay repro (seed/hash-order flaky):
- BEFORE fix: RED 5/12 (~42%), victims varied (`block:root-layout`, …).
- AFTER fix: RED 8/14 (~57%) — **statistically unchanged**, identical duplicate
  signature (`("block:c1", 2)`, `("block:root-layout", 2)`,
  `("block:default-advice-rules", 2)`, …). The ghost persists.

Change REVERTED; `git diff core/incremental/merge_operator.rs` is empty. No
second speculative fix attempted (per time-box guardrail).

### Refined localization (why merge-consolidate CANNOT be the fix, a class result)
The surplus row is null-padded (`tags=[]`) and sits **beside** the correct row
(`tags=[Page]`). Those two rows have **different values** ⇒ different
`HashableRow`s ⇒ `consolidate()` (which sums weights per identical row) can
**never** cancel them — neither at merge input NOR at merge output. This rules
out consolidate-at-merge as a fix *class*, not just this placement. The ghost is
not a transient unconsolidated `[-r, +r]` pair crossing the merge; it is a
**genuinely-emitted, never-retracted null-padded row living in persistent
operator state**, surviving across commit passes order-dependently.

Mechanism now pinned one level deeper: on the δL update (a `block_raw` content
UPSERT = left-side update on an already-**matched** left row), the antijoin fails
to **retract** the null-padded row it previously emitted for that key when the
key was unmatched. I.e. the unmatched→matched transition of the left row does not
propagate a `-null_padded` retraction through the `Antijoin` R-term. The
consolidate machinery is irrelevant because the stale and the live rows are not
weight-cancellable.

### Recommended next step (supersedes session-2 step)
Instrument `AntijoinOperator` (`core/incremental/antijoin_operator.rs`), NOT the
merge:
1. Driver: `REPLAY_MODE=trace` on `test_ivm_block_matview_replay_auto...` to get
   the statement index of the first δL UPSERT that creates the ghost for the
   victim key.
2. At that pass, log the antijoin's per-key match-count / R_COUNT state
   (`match_counter_operator.rs` + antijoin R-term) for the victim `block_id`
   BEFORE and AFTER the δL update. Confirm the hypothesis: the left row is
   already matched (count ≥ 1) so the antijoin should emit nothing, yet a prior
   null-padded row for that key was never retracted when the count first went
   0→1 (order-dependent: the +match and the −null-pad land in different passes
   through the SHARED agg matview, and the retraction is dropped).
3. Fix belongs in antijoin retraction on the match-count 0→1 edge (emit the
   pending `-null_padded`), or in how the inner-join/antijoin split handles a
   left-row update whose match status is unchanged but whose payload changed.
4. Gate stays: `auto` replay repro reliably green across ~15 runs, then the
   holon forced-weight keystone.

## Recommended next step (original)

1. Extend `matview_duplicate_row_repro.rs` with the doc-page shape
   (block already carrying a `Page` tag, then a `requires` edit) — verify it
   still reproduces after the chained-matview refactor; it is the minimal
   non-UI reproducer.
2. Debug incremental maintenance of the per-junction agg matviews +
   the `block` LEFT-JOIN in `schema_modules.rs` under Turso IVM: confirm
   whether the stale row lives in `block_tags_agg` (GROUP BY not collapsing
   incrementally) or in the top `block` LEFT-JOIN join-key maintenance.
3. Fix in Turso IVM / matview synthesis; keep the composed keystone forced-weight
   run above as the acceptance gate.
