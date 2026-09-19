---
id: 2026-09-19-set-field-on-missing-block-succeeds-silently
date: 2026-09-19
gap: ORACLE
secondary: COVERAGE
status: OPEN
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

Discovered through `dense_patch`, but NOT specific to it — see
`2026-09-17-dense-patch-apply-is-not-atomic` for that tool's own
partial-apply class. This entry is the underlying write-layer defect and would
outlive any fix to the patch applier.
