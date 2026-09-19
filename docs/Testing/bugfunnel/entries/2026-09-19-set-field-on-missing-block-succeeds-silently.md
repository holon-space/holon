---
id: 2026-09-19-set-field-on-missing-block-succeeds-silently
date: 2026-09-19
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  `set_field` against a block id that does not exist reports SUCCESS and writes
  nothing, so `dense_patch` returns `applied: true, updated: 1` for a state
  change on a block that is not in the store.
---

## Bug

Found by the VERIFIER of lane `dense-patch-atomic` while probing the
partial-apply reporting (`lane-logs/dense-patch-atomic-verify.md`, probe log
`verify-01-probes.log`). Driving `dense_patch` with a `SetState` op naming
`block:ghost` — an id no row carries — returns

```
applied: true, updated: 1
```

Nothing is written. The caller is told its state change landed.

This is a FAIL-LOUD violation of the first order: CLAUDE.md ranks "silently
degrades to look fine" last, and an agent acting on `updated: 1` will believe
a task was moved to DONE when it was not.

## Root cause

`SqlOperationProvider`'s single-op `set_field` arm
(`crates/holon/src/core/sql_operation_provider.rs:3460`) resolves the id, the
field and the value, checks the write route, and issues its UPDATE. It never
establishes that the row exists, and SQL grants an UPDATE against a missing
row silently — zero rows, no error.

The codebase already knows this and already fixed it ONE LAYER OVER. The batch
path carries the postcondition as
`assert_updated_rows_exist` (`sql_operation_provider.rs:1812`), whose own doc
comment states the mechanism in as many words:

> SQL grants an UPDATE against a missing row silently — zero rows, no error —
> so a sink row lost behind the caller's back stays lost and the caller goes on
> re-emitting UPDATEs that do nothing.

So the defect is not an unknown; it is an asymmetry. The batch seam asserts the
postcondition, the single-op seam does not, and `set_field` is the single-op
seam that `dense_patch`, the org write-back and every interactive edit use.

## Missing piece

**ORACLE.** No invariant anywhere asserts that a write reports failure when its
target does not exist. The keystone generates edits against blocks it created,
so the missing-row case is never generated at all, and the property that would
catch it — "an operation that changed no row returns `Err`" — is unwritten.

**Secondary COVERAGE.** The op-level tests for `set_field` all seed the row
first, so the arm's behaviour on an absent row is untested in either direction.

## Remedy

FIXED by ruling D147.a (Martin, 2026-09-19) in lane `set-field-assert`:
candidate 1 below, the post-check, with the row count taken from the driver so
no SELECT is added to the interaction path.

- `SqlOperationProvider::assert_row_matched` refuses a `set_field` whose UPDATE
  matched zero rows, naming the id, the field and the table. It is called on
  every leg of the arm that issues an UPDATE: the column and property-bag
  write, the `parent_id` write (which runs in a transaction for the deferred
  FK) and the rich-content Object write.
- `DbHandle::transaction_changes` was added beside `transaction` so the
  `parent_id` leg can read its changed-row count; `transaction` keeps its
  `Result<()>` signature and no caller moved.
- Red (assert inverted to `if true`, all four ghost writes return
  `Ok(OperationResult { .. })`, positive control passes):
  `lane-logs/01-red-inverted.log` — `5 tests run: 1 passed, 4 failed`. The same
  log is the teeth-by-inversion proof; the file was restored byte-for-byte
  (sha256 in `lane-logs/00-pristine-sha256.txt` and
  `lane-logs/02-restored-sha256.txt`).
- Green: `lane-logs/03-green-setfield.log` — `30 tests run: 30 passed`.
- Blast radius, the `-p holon -p holon-app` suite the entry asked for:
  `lane-logs/05-gate-nofailfast.log` — `799 tests run: 794 passed, 5 failed`,
  the 5 being the pre-existing `e2e_backend_engine_test` matview reds ("cannot
  modify materialized view block"), unrelated to this write path. No caller was
  relying on the no-op.
- Pinned by `crates/holon/src/core/set_field_missing_subject_test.rs` and
  stated as invariant 15 in `docs/Architecture/Model.md`.

NOT covered by this fix: the EDGE-field leg of the same arm returns before the
row UPDATE, so there is no changed-row count to read and a ghost subject writes
an orphan junction row. Filed separately as
`2026-09-20-set-field-on-an-edge-field-of-a-missing-block-writes-an-orphan-junction-row`
(OPEN) and named as the exception in invariant 15.

The `dense_patch`-level assertion the entry also asks for is NOT in this lane:
`execute_operation` surfaces the new `Err` as the MCP tool error
(`frontends/mcp/src/tools.rs:1496`), so a ghost id now fails loudly, but the
applier's own counting is lane `dense-patch-atomic`'s subject.

Original analysis of the two candidates, kept for the record:

OPEN — deliberately not fixed in lane `dense-patch-atomic`, because the choice
between the two candidate fixes is a latency decision on a hot path and is not
the lane's to make unilaterally:

1. **Post-check the UPDATE's rows-affected** and return `Err` when it is zero.
   Costs nothing extra on the happy path, but needs the row count to be
   available from the write, which the current `db_handle` write helper does
   not surface at this call site.
2. **Pre-read the row's existence** before writing, the same shape
   `assert_updated_rows_exist` uses. Simple and symmetric with the batch path,
   but adds one SELECT to EVERY `set_field` — and `set_field` is on the
   interaction path the p95 < 200ms SLO governs, so it needs a measurement
   before it is adopted, not an assumption.

Either way the fix is red-first: a test driving `set_field` at a nonexistent id
and demanding `Err`, plus the `dense_patch` level assertion that a `SetState`
naming an absent block is refused rather than counted as `updated`.

Blast radius to clear before landing: every `set_field` caller in the
workspace. A caller that currently relies on a no-op for a missing row would
start failing, which is the point, but the run that proves which callers those
are is a full `-p holon -p holon-app` suite (D64.a) and was not run here.

## Relation to other entries

UNMASKED by this fix:
`2026-09-20-doc-metadata-sync-writes-content-type-at-the-sql-projection` — the
first defect the new assert caught. Document-metadata sync had been writing a
doc-root's `content_type` at the SQL projection under Loro authority, matching
zero rows on every boot; the assert turned that silent no-op into a loud
ingest failure.

Discovered through `dense_patch`, but NOT specific to it — see
`2026-09-17-dense-patch-apply-is-not-atomic` for that tool's own
partial-apply class. This entry is the underlying write-layer defect and would
outlive any fix to the patch applier.
