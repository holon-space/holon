# Phantom-Loro-exists — root cause identified at startup

Date: 2026-05-11

## TL;DR

The directed instrumentation from
`devlog/2026-05-11-phantom-loro-exists-investigation.md` reproduced
the phantom-Loro-exists symptom on the **startup** code path
(pre-populated org file at builder time). The smoking gun is
`seed_loro_from_persistent_store` racing with `OrgFileWatcher`'s
initial scan: the watcher writes blocks to SQL *before* the
`LoroSyncControllerHandle` factory runs `seed_loro_from_persistent_store`,
so the seed reads those just-written blocks back out of SQL and
creates them in Loro in **HashMap-iteration order**. By the time the
matching CDC `Created` events reach `LoroSyncController`, the blocks
are already in the Loro tree and `apply_create` takes its
early-bypass to `apply_update_with_backend`.

The PBT's mid-test `BulkExternalAdd` may surface a *different* race
(or the same one if OrgFileWatcher re-fires `on_file_changed` for any
reason). The startup path is now fixed-cause; the mid-test path needs
its own pass.

## How it was found

Three single-line `tracing::error!("[PHANTOM-LORO-TRACE] …")` traces
plus a focused integration test:

1. **Write-side trace** in `LoroBlockOperations::create` (the
   documented "not wired as OperationProvider" path) — *did not
   fire*, ruling it out as a writer.
2. **Snapshot-existence trace** in
   `LoroDocumentStore::get_global_doc` — fired with `exists=false`,
   ruling out stale snapshot bleeding across test cases.
3. **Slow-path match trace** in
   `LoroBackend::find_tree_id_by_stable_id` — fired for every bulk
   block, revealing the matched TreeIDs.
4. **Seed-time create trace** in `seed_loro_from_persistent_store::apply_seed_row`
   — fired for every bulk block, **before** any CDC event arrived.

The focused test `phantom_loro_exists_repro::bulk_add_five_siblings_under_one_parent_at_startup`
pre-populates a `bulk.org` with 5 sibling blocks and runs to engine
startup. With the four traces above, the order of events on stderr
is:

```
[PHANTOM-LORO-TRACE] LoroDocumentStore::get_global_doc — snapshot_path=…/.loro/holon_tree.loro exists=false
[PHANTOM-LORO-TRACE] apply_seed_row about to create_block id=block:<doc-uuid> parent_id_raw=sentinel:no_parent
[PHANTOM-LORO-TRACE] apply_seed_row about to create_block id=block:bulk-0-0 parent_id_raw=block:<doc-uuid>
[PHANTOM-LORO-TRACE] apply_seed_row about to create_block id=block:bulk-0-3 parent_id_raw=block:<doc-uuid>
[PHANTOM-LORO-TRACE] apply_seed_row about to create_block id=block:bulk-0-1 parent_id_raw=block:<doc-uuid>
[PHANTOM-LORO-TRACE] apply_seed_row about to create_block id=block:bulk-0-2 parent_id_raw=block:<doc-uuid>
[PHANTOM-LORO-TRACE] apply_seed_row about to create_block id=block:bulk-0-4 parent_id_raw=block:<doc-uuid>
…
[PHANTOM-LORO-TRACE] find_tree_id_by_stable_id matched stable_id=bulk-0-0 tree_id=TreeID { peer: <local>, counter: 13 } …
[PHANTOM-LORO-TRACE] find_tree_id_by_stable_id matched stable_id=bulk-0-1 tree_id=TreeID { peer: <local>, counter: 41 }
[PHANTOM-LORO-TRACE] find_tree_id_by_stable_id matched stable_id=bulk-0-2 tree_id=TreeID { peer: <local>, counter: 55 }
…
```

Two facts to highlight:

- **Iteration order is scrambled.** The seed iterates rows in
  HashMap order (the `Vec<&HashMap<…>>::iter()` collects in query
  order, but the query order itself is ambiguous when all 5 blocks
  share `created_at` — see Why below).
- **All TreeIDs use the local peer.** Not a peer import.

## Why this happens

The startup sequence in
`crates/holon/src/sync/loro_module.rs::register_services`:

1. `OrgModeModule` factory runs first (registered earlier in DI).
   It builds `OrgSyncController` + `OrgFileWatcher` and spawns a
   background task that runs the initial scan
   (`crates/holon-orgmode/src/di.rs:842-870`). The initial scan
   calls `controller.on_file_changed(...)` for every pre-existing
   `.org` file — synchronously inside the spawned task, but
   awaiting the DB writes serially.

2. `LoroModule`'s `LoroSyncControllerHandle` factory runs
   *after* `LoroModule::configure` finishes registering. The
   factory body executes:

   ```rust
   seed_loro_from_persistent_store(&doc_store_arc, &db_handle).await
   ```

   `seed_loro_from_persistent_store` issues
   `SELECT id, parent_id, content, content_type, source_language, properties FROM <BLOCK_READ_TABLE> ORDER BY created_at ASC`.

   For org-sourced blocks, every block in a single `execute_batch_with_origin`
   call gets the SAME `created_at = now_millis()` (set by
   `build_block_params`). With a deterministic secondary sort key
   absent, SQLite returns rows in unspecified order.

3. `apply_seed_row` calls `backend.create_block(parent_uri, content, Some(block_id_uri))`
   for each row. Loro's `tree.create(parent)` appends a new child
   at the end of the parent's children list; the order of
   children is the order of these calls. Since the calls happen
   in unspecified row order, the Loro tree's children order is
   non-deterministic.

4. `LoroSyncController` finally starts. Its EventBus subscriber
   pulls the unprocessed CDC events from the initial scan
   (replay pass — see
   `crates/holon/src/sync/turso_event_bus.rs::subscribe`).

5. `apply_create` for each replayed event finds the block
   already in Loro (seed put it there) and takes the bypass to
   `apply_update_with_backend`. The bypass path calls
   `update_block_position(target, parent, after_id)` with the
   typed positional intent, which dispatches `tree.mov_after`.
   This **does** reorder the tree — but only if the typed
   positional intent is present, AND if the previous sibling
   referenced by `after_id` exists at the time of the mov_after.

If the OrgSync replay processes blocks in document order, each
mov_after operates against a predecessor that the previous iteration
already moved, so the final order should converge. So why does the
test still fail in production scenarios? Two candidate residuals
deserve their own trace passes:

- The replayed events arrive in `(created_at, id)` order — same
  ambiguity as the seed query.
- `apply_update_with_backend`'s mov_after only fires when
  `event.position_after_block_id.is_some()`. For events generated
  by OrgSync (which always emits `after_block_id`) this is fine,
  but the `prev_id` chain has gaps if the predecessor isn't yet
  in the tree.

## Recommended fix shape

This is a writeup, not an implementation PR. Three viable directions:

1. **Drop the seed entirely for org-routed blocks.** Let the
   inbound CDC path do all the writing. Blocks that bypass
   `OperationProvider` (notably `seed_default_layout`'s layout
   panel + sidebar entities, which write SQL directly) still
   need the seed — keep seed for *those* but skip rows that have
   already produced CDC events. Mechanically: the seed could
   read the events table for unprocessed `loro` events and skip
   any block whose `aggregate_id` already appears there.

2. **Make the seed deterministic and disjoint from CDC.** Add
   `, id ASC` (or `, sort_key ASC NULLS LAST, id ASC`) as a
   secondary sort to the `ORDER BY` in
   `seed_loro_from_persistent_store`. This guarantees a stable
   order but does NOT guarantee *document* order — `id` is the
   block's UUID or org `:ID:`, neither of which reflects
   position. Better: order by `sort_key ASC` first (the
   fractional index reflects intended document order).

3. **Gate the seed behind a barrier that the inbound runtime
   knows about.** Have the seed mark each block it creates with
   a sidecar flag (or insert a `block.created` event with
   `processed_by_loro = 1` to suppress replay). When apply_create
   then finds the block already exists, it can treat the bypass
   as expected rather than racing.

Option 2 is the smallest fix and matches the precedent set by
Phase 3.7 (typed positional intent via `position_after_block_id`).
The remaining race surface — apply_update_with_backend's
mov_after running before the predecessor exists — is structurally
identical to the gen-strategy-mismatch class Phase 3.7 closed; the
fix would be in the same direction (defer the mov_after until the
predecessor is in the tree, or ensure document-order replay).

## Open question on the PBT flake

The PBT's `BulkExternalAdd` writes the org file *after* startup
completes. The seed has long run by then. So the PBT flake is
**probably** a different race — possibly:

- `OrgFileWatcher` firing `on_file_changed` *twice* for the same
  file write (FSEvents emits multiple events for atomic writes
  on macOS), causing two batches with the same blocks. The
  second batch's `on_file_changed` reads `last_projection`
  (which was just updated by the first batch's success path) so
  it should see no diff and exit early. But race conditions in
  the `last_projection.insert` timing could leave a window where
  the second batch processes the same blocks fresh.
- The outbound projector firing a redundant create event due to
  `diff_snapshots_to_ops` over-reporting.

The next session should re-run the focused test with
`write_org_file` *mid-test* (after `start_app` completes) — the
existing failing variant `bulk_add_five_siblings_under_one_parent`
in `phantom_loro_exists_repro.rs` (which writes mid-test) currently
times out without firing seed traces. Resolving that test will
likely surface the PBT-specific phantom path.

## Files modified for this investigation

- `crates/holon/src/sync/loro_block_operations.rs` — `tracing::error!`
  at top of `LoroBlockOperations::create` + bare-`_` cleanups for
  pre-existing internal helper params.
- `crates/holon/src/sync/loro_document_store.rs` —
  `tracing::error!` at snapshot-exists check + bare-`_` cleanups +
  `ALLOW(compatibility)` for legacy aliases.
- `crates/holon/src/api/loro_backend.rs` — `tracing::error!` on
  `find_tree_id_by_stable_id` matched-node path.
- `crates/holon/src/sync/loro_module.rs::apply_seed_row` —
  `tracing::error!` before the `backend.create_block(...)` call.
- `crates/holon-integration-tests/tests/phantom_loro_exists_repro.rs`
  — new focused test that drives the production OrgSyncController +
  LoroSyncController flow with a 5-block bulk org file.

All four `[PHANTOM-LORO-TRACE]` traces remain in place. They're
gated by `tracing` levels (`error!`) so they only show up under
`RUST_LOG=error` (or wider). Leave them in until the PBT-side
flake is also resolved; remove in a follow-up PR with the actual
fix.

## Roadmap status update

| # | Item | Status |
|---|---|---|
| 1 | Resolve phantom-Loro-exists flake | **PARTIAL — startup root cause found; PBT mid-test path still open** |
| 2 | Flip the gate | blocked on (1) full close |
| 3 | Gate integration test | blocked on (2) |
| 4 | Retire sort_key write path entirely | blocked on (2) |
| 5 | Typed `_routing_doc_uri` | LANDED |
| 6 | Chord-op direct positioning via cell registry | blocked on (4) |
| 7 | archlint rule for new `_routing_*` payload keys | LANDED |
