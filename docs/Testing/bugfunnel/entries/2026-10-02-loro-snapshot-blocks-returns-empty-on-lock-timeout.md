---
id: 2026-10-02-loro-snapshot-blocks-returns-empty-on-lock-timeout
date: 2026-10-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  `LoroBackend::snapshot_blocks` returns an empty map when the doc read lock times out,
  so a caller cannot tell "no blocks" from "could not read".
---

## Bug

A read-only architecture study (2026-10-02, D26.b census, item K8 "silent fallbacks") found
this site. Nothing ran; this comes from reading the code.

## Root cause

`snapshot_blocks` (`crates/holon-loro/src/loro_backend.rs:4377-4381`) is
`with_read(..).unwrap_or_default()`. The closure cannot fail, so the only error it hides is the
`DocLock::read` timeout (`crates/holon-loro/src/doc_lock.rs:278-289`, "timed out ... waiting
for the doc read lock"). That error becomes an empty `HashMap`.

Reach today: no production code calls it. The only in-crate caller is
`diff_and_emit_after_import` (`loro_backend.rs:4611-4615`), and nothing calls that function. If
it were used, an empty `after` snapshot would emit a `Change::Deleted` for every block. The
other callers are tests that count nodes
(`crates/holon-integration-tests/tests/loro_suite/loro_restart_unseeded_vault.rs:184-248`,
`crates/holon-integration-tests/tests/cook_vault_ingest.rs:1398`,
`crates/holon-app/tests/org_store_org_round_trip.rs:472`). A lock timeout there reads as
"0 blocks", not as an error. So the defect is latent in production, but it can make those test
oracles lie.

## Missing piece

No test holds the doc write lock past `LOCK_WAIT_BUDGET` while calling `snapshot_blocks`. The
fallback is never exercised.

## Remedy

OPEN. Rung that closes the gap: a `holon-loro` unit test that holds the doc write lock on
another thread past `LOCK_WAIT_BUDGET` and calls `snapshot_blocks`. It asserts an `Err` naming
the doc. It goes red today because it gets `Ok(empty)`. Fix direction: return
`Result<HashMap<..>, ApiError>`, the same shape as `projected_blocks` (`:4388-4396`). Delete
`diff_and_emit_after_import` if it stays without a caller.
