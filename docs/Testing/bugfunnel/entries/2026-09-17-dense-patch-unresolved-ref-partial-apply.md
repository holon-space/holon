---
id: 2026-09-17-dense-patch-unresolved-ref-partial-apply
date: 2026-09-17
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  `dense_patch` resolved an op's reference to a new block the plan never
  creates to the EMPTY STRING and dispatched the batch anyway, so a plan that
  could not apply in full was applied in part and reported as success.
---

## Bug

Follow-up to the orchestrator report triaged as
`2026-09-17-dense-patch-apply-is-not-atomic` ("dense_patch applied its batch
and then returned an error"): that entry refuted the two named gate errors as
post-dispatch, and named the real class — a batch with no transaction, where a
mid-loop failure leaves earlier ops committed. This entry is the concrete,
reproducible route into that class.

Lane: `fix-mcp-tool-defects` (executor), base `bd8b719a103b`.

## Root cause

The applier resolved references through a closure whose `New` arm fell back to
a default:

```rust
PRef::New(t) => new_ids.get(t).cloned().unwrap_or_default(),
```

`unwrap_or_default()` on a `String` is the empty string, so an op naming a
block the plan never creates was dispatched with an empty `after_block_id` /
`parent_id` instead of being refused. Measured on a real engine with a
deliberately dangling reference (`PRef::New(7)`):

```
a plan naming a block it never creates must be refused: AppliedCounts { created: 2, updated: 0, moved: 0, deleted: 0 }
```

Two rows landed, one of them positioned against nothing, and the tool
returned `applied: true`. Red log: `lane-logs/05-red-A2-C.log`.

This is the silent-fallback shape CLAUDE.md forbids: the failure was
representable and was papered over rather than surfaced.

## Missing piece

**ORACLE.** `frontends/mcp/tests/dense_patch_pbt.rs` asserts heavily on
`plan_patch` — the pure planner — and nothing on the applier. `plan_patch`
happens to emit only consistent plans today, so the applier's
`unwrap_or_default()` is unreachable through `dense_patch`'s public input…
which is exactly why no invariant caught it: the property that matters is
"the applier refuses a plan it cannot apply in full", and it was never
asserted at the applier's own boundary.

**Secondary COVERAGE.** The planner's consistency is a coincidence of today's
emission order, not a checked obligation, so a future planner change reaches
this arm.

## Remedy

FIXED, `frontends/mcp/src/tools.rs`. The applier is now its own function,
`apply_plan`, which runs `plan_block_ids` to completion FIRST: every new
block's identity is minted up front (keeping both the bare uuid the `ID`
property carries and the `block:<uuid>` URI references resolve to) and every
`Ref::New` in every op is checked against it. A plan that cannot apply in full
is refused with `invalid_params` naming the unresolved reference, before the
first dispatch. `minted()` returns a loud `internal_error` for the
now-unreachable lookup miss, so no path can produce an empty id.

Pinned by `mod dense_patch_atomicity_tests`: the dangling-reference plan must
be refused AND leave the write authority empty, plus a happy-path control
proving the refusal is the plan's fault and not the applier's.

Not closed by this fix: an ENGINE-level failure mid-loop (a DB error on the
third op) still leaves the first two committed — the batch has no transaction.
That remains OPEN in `2026-09-17-dense-patch-apply-is-not-atomic`. Closing it
needs one atomic dispatch of the whole plan (pre-minted ids make the ops
independent, so `OriginTaggedWrites::execute_batch_with_origin` is the seam),
which is an architecture change this lane did not take.
