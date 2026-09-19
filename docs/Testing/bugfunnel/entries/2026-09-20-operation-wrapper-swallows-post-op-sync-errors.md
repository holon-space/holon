---
id: 2026-09-20-operation-wrapper-swallows-post-op-sync-errors
date: 2026-09-20
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  OperationWrapper reports a write as successful when the post-operation sync
  it owns failed, downgrading the failure to a tracing::warn, so a UI write
  that never reached the org file on disk looks like it landed.
---

## Bug

Found by the fresh-context verifier of lane `dense-revert-guard` (round 2,
`lane-logs/dense-revert-guard-verify-r2.md`, item 5) while checking whether the
lane's `NoSync` test stub hid anything production would surface. It does not —
the wrapper already hides it for every provider. Pre-existing and untouched by
that lane, recorded here rather than fixed inside it.

## Root cause

`OperationWrapper::execute_operation`
(`crates/holon-core/src/operation_wrapper.rs:105-114`):

```rust
if let Err(e) = sync_provider.sync_changes(&result.changes).await {
    tracing::warn!("[OperationWrapper] Post-operation sync failed for {}.{}: {}", …);
}
```

`Ok(result)` is returned regardless. The production wrapper's sync provider is
`OrgModeSyncProvider` (`crates/holon-app/src/turso_seams.rs:984`), so the write
that failed is the org write-back: the store has the change and the file on
disk does not, and the caller is told the operation succeeded. This is the
shape `CLAUDE.md` names as the one never to take — silently degrading to look
fine.

The same function's `sync` branch a few lines above propagates with `?`, so the
two failure paths of one provider disagree about whether a sync failure is an
error.

## Missing piece

**ORACLE.** No invariant judges what a provider's post-operation sync did.
Fault injection at that seam is generatable — the wrapper takes its sync
provider by injection, so a failing stub is one registration — and nothing
anywhere asserts that a failed write-back reaches the caller.

## Remedy

OPEN. Not fixed here: the wrapper is on every wired write path in both modes,
and turning a warning into an error changes what every frontend sees on a
transient org-write failure. That is a ruling about disclosure policy (fail the
write, or return success with a degraded-mode banner per the error-handling
priority order), not a lane decision.
