# The `holon` crate's integration reds — triage, 2026-09-01

Base: `main` = `ed38a4dae833`. Lane: `reds-triage`.

The `holon` crate's integration tests are run by **no gate** — the landing gate
runs `holon-app` only. 26 failures had accumulated there unseen. This document
classifies every one of them and records what was done.

**Note on invocation:** `-p holon` alone does not compile these tests; the
`test-helpers` feature is only unified when `holon-app` is in the same
invocation. Always use
`cargo nextest run -p holon-kitchen -p holon-core -p holon -p holon-app`.

## Result

| | Before | After |
|---|---|---|
| Failed | 25 | 7 |
| Timed out | 1 | 0 |
| Wall clock | 227.905s | 248.400s |

- Before: `lane-logs/ab-holon-main.nextest.log` —
  `Summary [ 227.905s] 885 tests run: 859 passed (8 slow, 1 leaky), 25 failed, 1 timed out, 5 skipped`
- After: `lane-logs/gate-full.nextest.txt` —
  `Summary [ 248.400s] 884 tests run: 877 passed (6 slow), 7 failed, 5 skipped`

All 7 remaining failures are classified below; **5 of them are the registered
pre-existing matview reds** and the other 2 are deliberate/dead-suite reds with
deletion proposals.

## Per-test table

| # | Group | Test | Class | Cause | Action |
|---|---|---|---|---|---|
| 1 | A | `integration_tests test_basic_two_peer_sync` | DEAD/TEST-ONLY PATH | ALPN never registered (below) | FIXED |
| 2 | A | `integration_tests test_bidirectional_sync` | DEAD/TEST-ONLY PATH | " | FIXED |
| 3 | A | `integration_tests test_three_peer_synchronization` | DEAD/TEST-ONLY PATH | " | FIXED |
| 4 | A | `integration_tests test_large_document_sync` | DEAD/TEST-ONLY PATH | " | FIXED |
| 5 | A | `integration_tests test_multiple_containers` | DEAD/TEST-ONLY PATH | " | FIXED |
| 6 | A | `integration_tests test_sequential_sync_sessions` | DEAD/TEST-ONLY PATH | " | FIXED |
| 7 | A | `integration_tests test_empty_document_sync` | DEAD/TEST-ONLY PATH | " | FIXED |
| 8 | A | `integration_tests test_rapid_sequential_edits` | DEAD/TEST-ONLY PATH | " | FIXED |
| 9 | B | `reliability_tests test_multiple_sequential_accepts` | DEAD/TEST-ONLY PATH | " | FIXED |
| 10 | B | `reliability_tests test_update_after_sync` | DEAD/TEST-ONLY PATH | " | FIXED |
| 11 | B | `reliability_tests test_sync_with_empty_peer` | STALE-ORACLE | asserted sync transfers **nothing** | FIXED (oracle inverted) |
| 12 | B | `stress_tests test_many_small_containers` | DEAD/TEST-ONLY PATH | ALPN | FIXED |
| 13 | B | `stress_tests test_sync_latency_measurement` | DEAD/TEST-ONLY PATH | ALPN | FIXED |
| 14 | B | `stress_tests test_large_batch_sync` | DEAD/TEST-ONLY PATH | ALPN | FIXED |
| 15 | B | `stress_tests test_parallel_sync_operations` (TIMEOUT) | STALE-ORACLE | accepted on the wrong endpoints | FIXED (accept topology) |
| 16 | E | `json_aggregation_e2e_test test_json_aggregation_includes_derived_columns` | STALE-ORACLE | asserted an enumerated `json_object` key list; transformer emits `json_object(*)` | FIXED (assert results, not SQL text) |
| 17 | E | `json_aggregation_e2e_test test_printf_sql_issue` | STALE-ORACLE | pinned an engine bug that is fixed | FIXED (assert success) |
| 18 | E | `json_aggregation_e2e_test test_union_query_with_json_object_via_backend_engine` | STALE-ORACLE | fixture tables lacked `_change_origin` | FIXED (fixture) |
| 19 | (repro) | `turso_storage_repros …cursor_filtered_main_panel_delivers_at_vault_scale` | ENVIRONMENT | load-sensitive budget: 853ms isolated vs 7.5s contended | NOT CHANGED — bugfunnel `2026-09-01-main-panel-delivery-budget-load-sensitive` |
| 20 | C | `turso_storage_pbt …test_turso_backend_state_machine` | **DEAD-SUITE** | `_version` column no longer exists in any schema | **STILL RED** — deletion proposed below |
| 21 | D | `create_page_from_link recreating_a_renamed_pages_old_name_yields_a_distinct_page` | DEAD-SUITE (pending-feature) | test pins an unimplemented end state | **STILL RED** — see below |
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

## Group A + B — the ALPN break in the test-only adapter (15 tests)

> **This was never a shipping bug.** `IrohSyncAdapter` has **zero production
> callers** — every call site is a test or `examples/peer_discovery.rs:49`.
> Production sharing goes through `SyncTransport`, whose iroh leg
> (`crates/holon-loro/src/iroh_advertiser.rs:151,154`) registers ALPNs correctly
> at bind; `crates/holon-loro/src/loro_backend.rs:4393` says so outright ("P2P
> sync requires IrohSyncAdapter (not wired to LoroBackend)"). The 15 rows are
> classified **DEAD/TEST-ONLY PATH**: fixed so the tests are real again, with
> deletion of the adapter pending Martin's D65.

`IrohSyncAdapter::new` bound its endpoint with **no ALPNs**
(`crates/holon-loro/src/iroh_sync_adapter.rs:462`,
`Endpoint::builder().bind()`). Under iroh 0.96 such an endpoint advertises no
protocol and rejects every peer at the handshake:

```
Error: aborted by peer: the cryptographic handshake failed:
  error 120: peer doesn't support any known protocol
```

The constructor that *does* register ALPNs, `new_with_alpns`, has **zero
callers anywhere in the workspace** — so this path had never worked for any
peer.

**Not attributable to a landing.** The file appears in only two revisions of
the available history (`2b2cd6953670 "WIP"`, `f9ebc9522a42 "feat: Backend"`) —
git/jj history here is squashed to 208 revisions, so file-level commit
archaeology is not possible. The break is a dependency-side API change (iroh
pinned at 0.96.0 in `Cargo.toml:152`, resolved 0.96.1), not an edit to this
file.

**Fix**: `accept_sync` registers the accepted doc's ALPN before accepting. The
doc is only known at that point, which is why `new()` could not have done it.

**A/B evidence** (same tree, only the fix differing):

- without the fix: `lane-logs/repro-ab.nextest.txt` —
  `Summary [ 120.705s] 48 tests run: 33 passed, 14 failed, 1 timed out, 0 skipped`
- with the fix: `lane-logs/green-ab.nextest.txt` —
  `Summary [   5.125s] 48 tests run: 47 passed (1 leaky), 1 failed, 0 skipped`

(The one remaining failure there was row 11, fixed after that run.)

### Two vacuous passes this uncovered

- `test_alpn_mismatch_detection` **passed for the wrong reason** — it only
  asserted that accept errors, and with no ALPN registered the *matching* case
  errored identically, so it passed with the fix present AND absent. It now runs
  **both legs**: a peer dialling `doc-beta` at a `doc-alpha` acceptor must be
  refused, and a peer dialling the matching id must sync. Removing the
  registration turns the second leg red.
- `test_sync_with_empty_peer` asserted `assert_eq!(text2, "")` — that sync
  transferred nothing. Corrected to `"Non-empty"`.

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

## Still-red rows — proposals

### Row 20 (group C) — DEAD-SUITE, propose deleting the version API

```
Failed to get version from Turso:
  DatabaseError("Failed to prepare query: Parse error: no such column: _version")
```

`TursoBackend::get_version` / `set_version`
(`crates/holon-turso/src/turso.rs:3885,3899`) read and write a `_version`
column. `TypeDefinition::to_create_table_sql`
(`crates/holon-api/src/entity.rs:640`) emits **only the declared fields** — no
`_version`, no `_dirty`. So no schema this code can create has the column, and
the PBT's model comment `// Turso adds _version column`
(`pbt_tests.rs:638`) describes a design that no longer exists.

`get_version` / `set_version` have **zero callers outside this PBT**, and
`StorageBackend` has exactly **one** implementation. Proposed deletion:

1. `crates/holon-core/src/storage/backend.rs:30,34` — the two trait methods.
2. `crates/holon-turso/src/turso.rs:3885-3906` — the impl.
3. `crates/holon/tests/turso_storage_pbt/pbt_tests.rs` — the `SetVersion`
   transition and the `versions` state (~16 sites).

Not done in this lane: a cross-crate public-API deletion is an architecture
call, and the brief asks DEAD-SUITE for a proposal. The alternative — adding
`_version` to `get_test_schema` — would make a dead API look alive and is
**not** recommended.

The same PBT's header also advertises "Dirty Tracking: MarkDirty, MarkClean,
GetDirty"; `_dirty` appears nowhere in `turso.rs`. Worth sweeping in the same
change.

### Row 21 (group D) — deliberate pending-feature red

The test's own panic message says so:

```
recreating page A must succeed (§5.3). Interim ADR 0029 D1b refuses it with
IdentityCollision instead; the end-state unique-random recreate is not
implemented
```

This is a spec-first test pinning an unimplemented end state. It is correctly
red. It should be either registered as a known red or `#[ignore]`d with the ADR
reference, so it stops reading as a regression — **Martin's call which.**

## Gating recommendation

**Add `-p holon` to the D43.a parallel nextest, not a nightly** — with two
conditions.

Measured cost: the full four-crate run is **248.400s** wall clock
(`/usr/bin/time -p`: `real 251.25`, `user 577.48` —
`lane-logs/gate-full.nextest.txt`). That is comparable to gates already run per
weave, and the alternative is what just happened: 26 reds accumulating
invisibly, one of which was a p2p sync path broken for every peer.

Conditions:

1. **Land the two still-red rows first** (20 and 21) — as deletion, `#[ignore]`,
   or registered known reds — or the gate is red on day one. The 5
   `e2e_backend_engine_test` reds are already registered.
2. **Row 19 needs a concurrency pin.** Its 5s budget holds at 853ms isolated but
   blows to 7.5s under full-suite parallelism. Put it in a nextest test-group
   with limited concurrency, or the gate will flake.

If those conditions are not acceptable, the fallback is a nightly tier judged
against this document rather than against zero failures.

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
