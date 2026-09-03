# The `holon` crate's integration reds — triage, 2026-09-01

Base: `main` = `ed38a4dae833`. Lane: `reds-triage`.

The `holon` crate's integration tests are run by **no gate** — the landing gate
runs `holon-app` only. 26 failures had accumulated there unseen. This document
classifies every one of them and records what was done. Martin's rulings D64.a,
D65.a and D66.a (2026-09-02) closed the last three.

**Note on invocation:** `-p holon` alone does not compile these tests; the
`test-helpers` feature is only unified when `holon-app` is in the same
invocation. Always use
`cargo nextest run -p holon-kitchen -p holon-core -p holon -p holon-app`.

## Result

| | Census | Now |
|---|---|---|
| Failed | 25 | 5 |
| Timed out | 1 | 0 |

- Census: `lane-logs/ab-holon-main.nextest.log` —
  `Summary [ 227.905s] 885 tests run: 859 passed (8 slow, 1 leaky), 25 failed, 1 timed out, 5 skipped`
- Now: `lane-logs/d2-nextest.txt` —
  `Summary [ 255.236s] 687 tests run: 681 passed (8 slow, 1 leaky), 6 failed, 6 skipped`

The 5 remaining reds are the registered `e2e_backend_engine_test` matview
failures. The 6th failure in that run,
`undo_concurrent_keystrokes concurrent_keystrokes_keep_every_undo_step`, is a
load flake: 3/3 green in isolation (`lane-logs/d2-undo-iso.txt`).

The test count drops from 885 to 687 because 21 adapter-driven tests were
deleted with the adapter and the suite is no longer run with `-p holon-kitchen
-p holon-core`.

## Per-test table

| # | Group | Test | Class | Cause | Action |
|---|---|---|---|---|---|
| 1 | A | `integration_tests test_basic_two_peer_sync` | DEAD/TEST-ONLY PATH | ALPN never registered (below) | DELETED |
| 2 | A | `integration_tests test_bidirectional_sync` | DEAD/TEST-ONLY PATH | " | DELETED |
| 3 | A | `integration_tests test_three_peer_synchronization` | DEAD/TEST-ONLY PATH | " | DELETED |
| 4 | A | `integration_tests test_large_document_sync` | DEAD/TEST-ONLY PATH | " | DELETED |
| 5 | A | `integration_tests test_multiple_containers` | DEAD/TEST-ONLY PATH | " | DELETED |
| 6 | A | `integration_tests test_sequential_sync_sessions` | DEAD/TEST-ONLY PATH | " | DELETED |
| 7 | A | `integration_tests test_empty_document_sync` | DEAD/TEST-ONLY PATH | " | DELETED |
| 8 | A | `integration_tests test_rapid_sequential_edits` | DEAD/TEST-ONLY PATH | " | DELETED |
| 9 | B | `reliability_tests test_multiple_sequential_accepts` | DEAD/TEST-ONLY PATH | " | DELETED |
| 10 | B | `reliability_tests test_update_after_sync` | DEAD/TEST-ONLY PATH | " | DELETED |
| 11 | B | `reliability_tests test_sync_with_empty_peer` | STALE-ORACLE | asserted sync transfers **nothing** | DELETED |
| 12 | B | `stress_tests test_many_small_containers` | DEAD/TEST-ONLY PATH | ALPN | DELETED |
| 13 | B | `stress_tests test_sync_latency_measurement` | DEAD/TEST-ONLY PATH | ALPN | DELETED |
| 14 | B | `stress_tests test_large_batch_sync` | DEAD/TEST-ONLY PATH | ALPN | DELETED |
| 15 | B | `stress_tests test_parallel_sync_operations` (TIMEOUT) | STALE-ORACLE | accepted on the wrong endpoints | DELETED |
| 16 | E | `json_aggregation_e2e_test test_json_aggregation_includes_derived_columns` | STALE-ORACLE | asserted an enumerated `json_object` key list; transformer emits `json_object(*)` | FIXED (assert results, not SQL text) |
| 17 | E | `json_aggregation_e2e_test test_printf_sql_issue` | STALE-ORACLE | pinned an engine bug that is fixed | FIXED (assert success) |
| 18 | E | `json_aggregation_e2e_test test_union_query_with_json_object_via_backend_engine` | STALE-ORACLE | fixture tables lacked `_change_origin` | FIXED (fixture) |
| 19 | (repro) | `turso_storage_repros …cursor_filtered_main_panel_delivers_at_vault_scale` | ENVIRONMENT | load-sensitive budget: 853ms isolated vs 7.5s contended | PINNED to a single-thread nextest test-group (D64.a); budget unchanged |
| 20 | C | `turso_storage_pbt …test_turso_backend_state_machine` | DEAD-SUITE | `_version` column exists in no schema | DELETED — the version API and its transitions are gone (D65.a) |
| 21 | D | `create_page_from_link recreating_a_renamed_pages_old_name_yields_a_distinct_page` | pending-feature | pins an unimplemented end state | `#[ignore]`d with the ADR 0029 D1b reason (D66.a) |
| 22-26 | F | `e2e_backend_engine_test` ×5 | KNOWN RED | "cannot modify materialized view block" | **STILL RED** — registered, base-attributed |

### Census correction

The brief listed **6** `e2e_backend_engine_test` reds and asked which the 6th
was. There are exactly **5**; they are the registered root-remodel-epic reds
(`test_basic_query_execution`, `test_create_and_delete_workflow`,
`test_multiple_operations_sequence`, `test_operation_triggers_stream_update`,
`test_query_and_watch_stream`), all failing with `Failed to insert test data:
Database error: Failed to prepare statement: Parse error: cannot modify
materialized view block`. The census row matches. The apparent 6th was row 19,
a different suite (`turso_storage_repros`).

## Group A + B — the legacy sync adapter is DELETED (15 tests)

`IrohSyncAdapter` and its `new_with_alpns` constructor no longer exist, and
neither do the 21 tests that drove them (11 in `integration_tests`, 5 in
`reliability_tests`, 5 in `stress_tests`) nor `examples/peer_discovery.rs`.

Production P2P sync is `SyncTransport` plus `iroh_advertiser`, which registers
its ALPNs at bind through `create_endpoint` / `create_endpoint_with_key`
(`crates/holon-loro/src/iroh_sync_adapter.rs:169,195`). That
`Endpoint::builder().alpns(...)` call is now the ONLY ALPN registration idiom in
the workspace — there is no post-bind `set_alpns` anywhere.

The module `crates/holon-loro/src/iroh_sync_adapter.rs` REMAINS: it holds the
production sync primitives (`create_endpoint`, `make_alpn`, `sync_doc_initiate`,
`sync_doc_handle_connection`, `SharedTreeSyncManager`, `IrohSync`), which
`iroh_advertiser` and `loro_share_backend` import. Only the legacy adapter went.

The three test files keep their 28 Loro-only tests (conflict convergence,
idempotency, snapshot consistency, boundary inserts, and so on); those never
touched the adapter.

## Group E — measured, all three were stale oracles

Row 16 is worth stating precisely because it could have been a product defect.
The transformer unconditionally emits `json_object(*)` over each branch CTE
(`crates/holon-turso/src/sql_parser.rs`, `make_json_select_from_cte`) rather
than an enumerated key list. The test asserted the *SQL text* contained
`'entity_name'` — but it already had result-level assertions further down that
the text assertions blocked. Removing the text assertions let the behavioural
oracle run, and it **passes**: `json_object(*)` does carry the CTE's derived
columns at runtime. Measured, not assumed
(`lane-logs/gate-full.nextest.txt`, `PASS [ 1.883s] (337/884)`).

Row 18: `_change_origin` is injected into every UNION branch for tables named
in the hardcoded `TABLES_WITH_CHANGE_ORIGIN` list (`sql_parser.rs:756-762`),
which includes `todoist_project` and `todoist_task`. The fixture created those
tables without the column. **Design smell worth a separate look:** a hardcoded
table-name list decides which tables are assumed to have a column — a
schema-driven lookup would make the fixture gap unrepresentable.

## Rows 20 and 21 — resolved

### Row 20 (group C) — the version API is DELETED

`StorageBackend` no longer declares `get_version` / `set_version`, `TursoBackend`
no longer implements them, and the `turso_storage_pbt` state machine no longer
carries a `SetVersion` transition, a `versions` reference map, or the
`_version` / `_dirty` bookkeeping its header used to advertise.

Nothing read that API outside the PBT, and `TypeDefinition::to_create_table_sql`
(`crates/holon-api/src/entity.rs:640`) emits only the declared fields — so no
schema the system can build has a `_version` column for it to query.

### Row 21 (group D) — pending feature, ignored with its reason

`recreating_a_renamed_pages_old_name_yields_a_distinct_page` carries
`#[ignore = "ADR 0029 D1b end-state pending: unique-random recreate not
implemented"]`. It pins §5.3's end state, which is not built: the interim
behaviour refuses the recreate with `IdentityCollision`. Remove the attribute
when the unique-random recreate lands.

## Gating recommendation

**Add `-p holon` to the D43.a parallel nextest, not a nightly.** Both blockers
are cleared: rows 20 and 21 are deleted and `#[ignore]`d, row 19 runs in the
`vault-scale-latency` test-group, and the only remaining reds are the 5
registered `e2e_backend_engine_test` matview failures.

Measured cost: `cargo nextest run --no-fail-fast -p holon -p holon-app` is
**255.236s** (`lane-logs/d2-nextest.txt`). That is comparable to gates already
run per weave, against the alternative this document exists to record — 26 reds
accumulating invisibly behind a suite no gate executed.

`-p holon` must always be invoked WITH `-p holon-app`: the `test-helpers`
feature is only unified when both are in the same invocation, and `-p holon`
alone does not compile these tests.

**One caveat for whoever wires the gate.**
`undo_concurrent_keystrokes concurrent_keystrokes_keep_every_undo_step` failed
once in ~4 full runs (7 undo steps instead of 3, and out of order) and has not
reproduced in 9 isolated runs. It is **not** the same kind of problem as row 19
and must not get the same treatment: row 19's oracle is a wall-clock budget, so
pinning concurrency removes noise without removing signal, whereas this test's
oracle is a correctness equality whose *subject* is concurrency — a
`max-threads = 1` group would delete the only condition under which the property
is exercised. Tracked as bugfunnel
`2026-09-02-concurrent-keystroke-undo-step-count-diverges` (ORACLE, OPEN) until
root-caused.

## Unexplained residual

The census run reports 885 tests run, the post-fix run 884. The three suites
this lane touched have **identical** test sets across both runs (48 unique
names each, verified by name-set diff), so the delta is not from these edits.
Not chased further.

## Delta — 2026-09-01, after adversarial verification

`reds-triage-verify.md` returned CONFIRMED with three defects; all addressed.

- **Classification corrected.** The 15 A+B rows read `REAL-DEFECT`; the adapter
  has zero production callers. Relabelled **DEAD/TEST-ONLY PATH** and the
  production path named in the section note above.
- **`Endpoint::set_alpns` REPLACES the protocol set** (iroh 0.96.1
  `endpoint.rs:757-760`), so `accept_sync(doc_b)` un-registered `doc_a`. The
  adapter now keeps a `BTreeSet` of registered ALPNs and sets the union
  (`register_alpn`). Red-first via a new two-doc test —
  `accepting_a_second_doc_keeps_the_first_docs_alpn_registered` failed with
  "cumulative-a was refused at the handshake — its ALPN was un-registered"
  (`lane-logs/red-cumulative.txt`), green after.
- **`display_name` had lost all coverage** when the SQL-text assertions went.
  Result-level assertions added for it and for `results.len() == 2`; teeth shown
  by breaking the key name (`lane-logs/probe-display.txt`, `FAIL [ 1.828s] (5/8)`)
  and restoring byte-for-byte.

Re-run: `lane-logs/d-sync-json.txt` —
`Summary [   6.360s] 57 tests run: 57 passed (1 leaky), 0 skipped`; fmt clean;
`just check-worker-wasm` 5/5; bugfunnel `586 entries, 0 problems`.
