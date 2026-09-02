---
id: 2026-09-02-loro-create-persist-oracle-assumes-synchronous-projection
date: 2026-09-02
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The loro-suite create-persist lock asserts a `block_raw` row exists the
  instant `block.create` returns, but the Loro to SQL projection is an
  asynchronous spawned task, so the test fails whenever the machine is loaded.
---

## Bug

`holon-integration-tests::loro_suite
loro_create_persists_prod_session::prod_session_create_block_persists_to_block_raw`
fails intermittently in the land battery:

```
crates/holon-integration-tests/tests/loro_suite/loro_create_persists_prod_session.rs:121
assertion `left == right` failed: under an existing block: block.create returned
success but NO row was in block_raw immediately after — success-before-persist
(fail-loud violation)
  left: 0
 right: 1
```

Found by the land-battery gate on the D67.a integration chain, not by any
routine run — the `loro-suite` recipe is a landing step and the weave gate does
not run it.

## Root cause

The oracle asserts a synchrony the product does not promise.

The test dispatches through the prod path
(`loro_create_persists_prod_session.rs:115`), then asserts at line 121 that the
row is queryable *the instant* `create` returns. But with CRDT enabled the
dispatch only mutates the Loro document; the `block_raw` write is performed by
a **separate spawned task**:

- `crates/holon-loro/src/loro_sync_controller.rs:396` — the `subscribe_root`
  callback appends the pending facts and calls `wake_for_callback.notify_one()`.
  That is all the dispatch does synchronously.
- `crates/holon-loro/src/loro_sync_controller.rs:422-425` —
  `let task = tokio::spawn(async move { … self.run_loop().await });` is the
  async boundary.
- `crates/holon-loro/src/loro_sync_controller.rs:446` — the loop body is
  `self.wake.notified().await`, then
  `on_loro_changed()` → `self.projection.project().await`
  (`loro_sync_controller.rs:459-460`), which is what finally writes `block_raw`.

So between `create` returning and the row existing there is a task wake, a
scheduler hop and a projection pass. Nothing orders them against the caller.
The test's own module doc claims "the row is present the instant `create`
returns AND after `wait_for_loro_quiescence`, so there is no async projection
race" — the second half is sound (line 129 awaits quiescence and line 130
re-asserts); the first half is the unsound part.

### Measurement: it tracks machine load, not any commit

Full `loro-suite` (12 tests in parallel, the battery's conditions),
`CARGO_BUILD_JOBS=4`, three trees:

| tree | contents | separate windows | interleaved, quiet window |
|---|---|---|---|
| MAIN | `4870faab4027` | 0/8 failed (suite ~3.1s) | 0/6 failed |
| REDS_TRIAGE | main + D65.a/D66.a/D64.a | **4/8 failed** (suite ~5.0s) | 0/6 failed |
| WITH_D67 | main + D67.a | **1/8 failed** (the failing run's suite took 9.1s; the seven passing ones ~3.2s) | 0/6 failed |

Run alone (`-E` by test name) it passes 3/3 on every tree in ~1.5s.

The per-tree rates in the left column were measured in different machine
windows and track the suite's wall time, not the tree. Interleaving the three
trees within ONE quiet window removes the confound: **all three pass 6/6**, all
at ~3.1s. In the battery's own failing run the test took 6.7s against 1.5s
isolated.

Nothing in the reds-triage diff touches this path: its two `loro_backend.rs`
hunks change only `connect_to_peer` / `accept_connections` error strings, and
its `.config/nextest.toml` addition scopes the `vault-scale-latency` group to
`test(cursor_filtered_main_panel_delivers_at_vault_scale)`, a `holon`-crate
test absent from this binary. D67.a is not in the read path either: the test's
`block_rows` helper queries `engine.db_handle().query(...)`, the raw handle, so
`apply_sql_transforms` never runs on it.

## Missing piece

No invariant distinguishes "the write was dropped" from "the write has not been
projected yet". The test tried to express the first by assuming the second is
impossible, and the product's own architecture (Loro is the authority, one
spawned projector) says it is not. So a real success-before-persist regression
and an ordinary loaded machine produce the identical red — the oracle cannot
tell the escape it exists for from noise.

The line-129 assertion after `wait_for_loro_quiescence` IS a sound lock on the
original 2026-07-21 dogfood bug (the projection loop was wedged, so the row
never arrived at all). The line-121 assertion adds no coverage of that escape
and subtracts trust from the suite.

## Remedy

FIXED by remedy 1 (Martin's ruling, 2026-09-02): the instant-row assertion is
deleted from
`crates/holon-integration-tests/tests/loro_suite/loro_create_persists_prod_session.rs`
and the surviving assertion — after `wait_for_loro_quiescence` — carries the
success-before-persist message, so a wedged projection still fails loudly by
exhausting the budget. The module doc now states the actual contract: with CRDT
enabled the Loro commit IS the persist, and the `block_raw` row is a projection
that is promised within the quiescence budget, not at the moment `create`
returns.

The two rejected alternatives, for the record: a tighter measured budget
(keeps latency pressure but needs a number chosen from load, not from a quiet
machine), and making `create` synchronous w.r.t. the first projection pass (a
product change with latency consequences for every write — a ruling, not a test
fix).
