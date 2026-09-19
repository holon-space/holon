---
id: 2026-09-19-rollback-measured-only-the-vault-document
date: 2026-09-19
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  dense_patch's rollback took its version from the vault document alone while a
  block write also routes to the device-local layout document and to shared
  subtree documents, so a batch write into one of those survived a rollback
  that reported a full rewind, and a peer writing there raised no refusal.
---

## Bug

Found by the fresh-context verifier of lane `dense-revert-guard` (report
`lane-logs/dense-revert-guard-verify.md`, defect 2, ranked MEDIUM-HIGH).

Measured (`lane-logs/verify-probe-ab.log:14-17`):

```
PROBE-B outcome=Ok(())
PROBE-B global_after=["block:root"]
PROBE-B layout_after=["block:layoutroot", "block:layoutop"]
PROBE-B layoutop_survived_despite_claimed_full_rollback=true
```

`rollback_to` returned `Ok`, so `dense_patch` told the caller the store was
back at its pre-patch state while the layout write was still there.

## Root cause

`LoroBlockOperations::global_doc` resolved `DocScope::Global` only, and both
halves of the rollback used it. Its comment claimed the layout document holds
no block a batch can touch. That is false:
`LoroBackend::resolve_write_target_sync`
(`crates/holon-loro/src/loro_backend.rs:2264-2267`) probes the LAYOUT document
FIRST, layout changes project into SQL alongside the vault document
(`crates/holon-loro/src/loro_sync_controller.rs:766-772`), and the same
resolver routes to shared subtree documents
(`scan_shared_for_tree_id`, `loro_backend.rs:2278`). The guard was equally
blind: a remote peer writing into a layout or shared document raised no
refusal.

## Missing piece

**ORACLE.** Every test in the lane wrote to the vault document only, so no case
distinguished "the authority was rolled back" from "one of its documents was".
The false comment stood because nothing exercised the claim.

## Remedy

FIXED. `AuthorityVersion` (`crates/holon-core/src/batch_rollback.rs`) is a map
over every routable document, and `LoroBlockOperations::routable_docs`
enumerates the same set the write resolver routes over: the vault document, the
layout document, and each shared subtree document from `SharedTreeStore`. Both
the guard and the revert cover all of them. Every refusal names the document it
concerns. A document that joined the routing set inside the window has no
pre-batch version and is refused rather than skipped.

Reverts are ordered after every check, so a refusal leaves the store untouched;
a revert that fails after another already succeeded is
`RollbackRefused::Incomplete`, which names what was reverted.

Pinned by `a_batch_write_into_the_layout_document_is_rolled_back_too` and
`an_intruding_layout_write_refuses_the_rollback_and_names_that_document`
(`crates/holon-loro/tests/batch_rollback_guard.rs`), which are this entry's
probe. Red by inversion with the enumeration cut back to the vault document:
`lane-logs/RED-4-vault-doc-only.log` — `left: ["block:layoutop",
"block:layoutroot"] right: ["block:layoutroot"]`. Green:
`lane-logs/d1-loro-guard-2.log`.
