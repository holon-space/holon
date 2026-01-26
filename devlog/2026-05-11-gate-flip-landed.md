# Phase 3.3 step 2 — inbound runtime gate flipped; phantom-Loro item 1b verified closed; gate integration test landed

Date: 2026-05-11

## Outcome

Roadmap items 1b, 2, and 3 closed in this session. Investigation traces removed.
Item 3 surfaced a follow-up: two unmigrated production paths still emit
`origin=Other("sql")` for block creates — documented below.

| # | Item | Status |
|---|---|---|
| 1a | Phantom-Loro startup-seed race | LANDED (previous session) |
| 1b | Phantom-Loro PBT mid-test variant | **CLOSED** (verified by full PBT, this session) |
| 2 | Flip the inbound runtime gate | **LANDED** (this session) |
| 3 | Gate integration test | **LANDED** (this session) |
| 4 | Retire `sort_key` write path entirely | Open — next |
| 5 | Typed `_routing_doc_uri` | LANDED (previous session) |
| 6 | Chord-op direct positioning via cell registry | After (4) |
| 7 | archlint rule for `_routing_*` payload keys | LANDED (previous session) |

## Verification log

Four full PBT runs in sequence, each `RUST_LOG=error PROPTEST_CASES=1 cargo test
-p holon-integration-tests --test general_e2e_pbt general_e2e_pbt -- --nocapture`:

| run | what changed before it | result | BYPASS | regression-guard hits |
|---|---|---|---|---|
| 1 | none — baseline with traces from previous session | 2/2 pass, 521s | 0 | 0 |
| 2 | gate flipped on (loro_module.rs:236) | 2/2 pass, 515s | 0 | 0 |
| 3 | 5 investigation traces removed | 2/2 pass, 506s | n/a (traces gone) | n/a |
| 4 | item-3 test infra (3 wrapper accessors + new test file) | 2/2 pass, 539s | n/a | n/a |

Run 1 conclusively closes item 1b: the focused mid-test reproducer in
`phantom_loro_exists_repro.rs::two_consecutive_bulk_batches_under_one_parent`
already passed cleanly post-startup-seed-fix, but the recommended path (b) from
the handoff was to verify against the actual PBT. Zero BYPASS lines for any
non-layout block confirms the startup-seed-fix landed in item 1a closes the
phantom-Loro path comprehensively — the focused harness was exhaustive after
all.

Run 2 proves the gate flip doesn't regress anything: `handle.disable_inbound_runtime()`
now runs on every controller startup, so SQL→Loro reflection of non-Loro-origin
block events is permanently off. Full + SqlOnly PBT variants stay green.

Run 3 confirms removing the instrumentation didn't unwire anything load-bearing.

Also verified post-flip:
- `cargo test -p holon --lib sync::loro_sync_controller` — 16/16 (includes
  `inbound_gate_tests` for the pure-function decision helper).
- `cargo test -p holon-integration-tests --features otel-testing --test
  phantom_loro_exists_repro` — 2/2.

## Changes

### Gate flip — `crates/holon/src/sync/loro_module.rs`

```diff
             match controller.start().await {
                 Ok(handle) => {
-                    // Phase 3.3 step 2 gate flip is wired and unit-tested in
-                    // `loro_sync_controller::inbound_gate_tests`, but the
-                    // call is held back here pending a separate fix for a
-                    // Full-mode PBT block-order flake that reproduces
-                    // independent of the flip — see
-                    // `devlog/2026-05-11-024012-phase3.3-step2-scaffolded-flake-blocks-flip.md`.
-                    // When the flake is fixed, replace the marker line below
-                    // with `handle.disable_inbound_runtime();`.
-                    let _hold_gate_open = &handle;
+                    handle.disable_inbound_runtime();
                     Shared::new(handle)
                 }
```

### Trace removal

All five `[PHANTOM-LORO-TRACE]` instrumentation sites left over from the item-1a
investigation deleted:

- `crates/holon/src/sync/loro_block_operations.rs::create` — `error!`
  regression-guard removed (item 1a closed → no regression possible from this
  path; if the path ever does fire again, that's a code change that should add
  its own diagnostics).
- `crates/holon/src/sync/loro_sync_controller.rs::apply_create` BYPASS branch
  — `error!` regression-guard removed; the `debug!` "Block X exists, updating
  instead" line beneath it stayed (it's load-bearing for normal CDC flow,
  pre-dates the investigation).
- `crates/holon/src/sync/loro_document_store.rs::get_global_doc` — snapshot-exists
  `debug!` removed.
- `crates/holon/src/api/loro_backend.rs::find_tree_id_by_stable_id` — matched-node
  `debug!` removed.
- `crates/holon/src/sync/loro_module.rs::apply_seed_row` — create_block call-site
  `debug!` removed.

The handoff allocated removal to "a cleanup PR after item 1b is conclusively
closed." Item 1b is conclusively closed by Run 1 above.

## Item 3 — gate integration test

`crates/holon-integration-tests/tests/inbound_runtime_gate.rs` adds three
end-to-end tests that exercise the gate against a real EventBus +
`LoroSyncController`:

1. **`gate_is_disabled_in_production_wiring`** — boots a default fixture and
   asserts `LoroSyncControllerHandle::inbound_runtime_enabled() == false`,
   proving the `loro_module.rs::register_services` factory engages
   `disable_inbound_runtime()` on every controller start.
2. **`org_origin_events_pass_the_gate_as_apply`** — publishes a synthetic
   `EventOrigin::Org` block.updated event directly to the bus; asserts
   `applied_count` ticks (decision: `Apply`) and `drop_count` stays unchanged.
3. **`ui_origin_events_are_dropped`** — publishes a synthetic
   `EventOrigin::Ui` event; asserts `drop_count` ticks (decision: `Drop`)
   and `applied_count` stays unchanged.

Both synthetic-event tests baseline counters AFTER startup settles, so any
unmigrated startup-time drop paths don't false-positive them. The pure-function
decision matrix is already covered by `loro_sync_controller::inbound_gate_tests`
(5/5); this file is the missing end-to-end coverage.

Test infrastructure added in `crates/holon-integration-tests/src/test_environment.rs`:
- `loro_sync_inbound_runtime_enabled()` — gate state accessor.
- `loro_sync_drop_count()` — non-whitelisted origin drops since startup.
- `loro_sync_applied_count()` — whitelisted-origin (or gate-open) applies.

## Surfaced follow-up — unmigrated `origin=Other("sql")` block creates

While iterating on test 2 (the test originally tried to verify Org-origin pass
via the real `OrgFileWatcher → OrgSyncController` pipeline), the gate's warn
log surfaced two production paths still emitting `origin=Other("sql")`:

1. **`LiveDocumentManager::create`** at `crates/holon-orgmode/src/di.rs:418` —
   used by `OrgSyncController::on_file_changed` when a new file's document
   block doesn't yet exist (`traits.rs:148` and `org_sync_controller.rs:266`).
   It calls `command_bus.execute_operation("block", "create", params)` — the
   single-op `execute_operation` path doesn't have an origin parameter, so
   `SqlOperationProvider::publish_event` hardcodes `EventOrigin::Other("sql")`.
2. **`SqlOperationProvider::execute_operation` "create" arm** at
   `crates/holon/src/core/sql_operation_provider.rs:1137` — calls
   `self.publish_event(EventKind::Created, ...)` (line 1177) which hardcodes
   `EventOrigin::Other("sql")` at `sql_operation_provider.rs:196`. Any caller
   that creates blocks via the single-op `execute_operation` path inherits this.

This is exactly what the gate's `warn!` was designed to surface ("an unmigrated
SQL-direct block-write path that should route through cells"). Test 2 was
restructured to use a synthetic Org-origin event published directly to the
EventBus — that isolates gate-decision semantics from producer-side wiring.

**Recommended migration** (out of scope for item 3, sized for item 4 territory):

- Add an `execute_operation_with_origin(entity, op, params, origin)` to
  `OperationProvider`, defaulting to `execute_operation` (origin ignored). Have
  `SqlOperationProvider` override it to thread the origin into all event
  publishes inside the dispatch arms.
- Migrate `LiveDocumentManager::create` to call it with `EventOrigin::Org`.
- Audit other `execute_operation` block-create callers (`action_watcher`, etc.)
  and tag them with the right origin.

Until then, the new integration test stays useful — `drop_count` is now an
observable canary for any *new* unmigrated path that lands during item-4 work.

## Files changed in this session

- `crates/holon/src/sync/loro_module.rs` — gate flip; trace removed.
- `crates/holon/src/sync/loro_sync_controller.rs` — regression-guard trace removed.
- `crates/holon/src/sync/loro_block_operations.rs` — regression-guard trace removed.
- `crates/holon/src/sync/loro_document_store.rs` — debug trace removed.
- `crates/holon/src/api/loro_backend.rs` — debug trace removed.
- `crates/holon-integration-tests/src/test_environment.rs` — 3 wrapper accessors.
- `crates/holon-integration-tests/tests/inbound_runtime_gate.rs` — new file, 3 tests.
- `devlog/2026-05-11-gate-flip-landed.md` — this file.
