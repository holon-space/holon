# SplitBlock fails because Loro inbound runtime isn't processing events

## Symptom

Production split fails with:

```
ERROR backend.execute_operation{operation.entity="block" operation.name="split_block"}:
  [BackendEngine] Operation 'split_block' on entity 'block' failed:
  create_block(block:78e3abf6-…): Internal error: Failed to create block:
  Cannot resolve parent URI to TreeID: block:cb7d94d4-75fa-4043-a6a7-a58e509b24e0
```

## Root cause

The Loro inbound runtime — the consumer that mirrors confirmed SQL events
into the Loro tree — never processes any events in this running session.

### Evidence (gathered via `holon-live` MCP)

| Table              | Query                                                              | Result |
|--------------------|--------------------------------------------------------------------|--------|
| `events`           | `COUNT(*)`                                                         | 1283   |
| `event_acks`       | `WHERE consumer = 'cache'`                                          | 1283 (caught up) |
| `event_acks`       | `WHERE consumer = 'org'`                                            | 1038 (caught up) |
| `event_acks`       | `WHERE consumer = 'loro'`                                           | **0**  |
| `events` for parent `block:cb7d94d4-…` | `block.created`, origin=`org`, status=`confirmed`        | 1 row, **no loro ack** |
| `events` for grandparent `block:db147710-…` | same                                                          | 1 row, **no loro ack** |
| `block_raw` for parent                  | present, content=`"Holon"`, parent_id=`block:db147710-…`              | exists |
| `mcp.list_loro_documents()`             | —                                                                  | error: `Loro is not enabled in this session` |

### Mechanics

1. The chord-op path for split is `traits.rs::BlockOperations::split_block` →
   `create_block_via_cells(parent, after, new_id, content)` →
   `LoroBackend::create_block(parent_uri, content, Some(new_id))` →
   `resolve_parent_tree_id(tree, id_cache, parent_uri)`.
2. `resolve_parent_tree_id` looks up the parent's `TreeID` via three paths:
   - parse the URI itself as a `TreeID` (only if it has the TreeID format,
     which a normal `block:<uuid>` URI doesn't);
   - `id_cache` lookup keyed by `parent_uri.id()`;
   - tree walk searching every alive node's `STABLE_ID` meta.
3. The Loro tree has never been seeded with org-parser-created blocks
   (because the inbound runtime never ran). Step 2 fails for any parent
   that came from `.org` ingestion → `Err("Cannot resolve parent URI to TreeID: …")`.
4. The error propagates up through `LoroBackend::create_block` →
   `create_block_via_cells` → `BlockOperations::split_block` →
   `BackendEngine::execute_operation`, surfacing as the user-visible error.

### Why the inbound runtime isn't acking events

Per `crates/holon/src/sync/loro_sync_controller.rs::start`:

- Subscribe to event_bus with filter `(AggregateType::Block, EventStatus::Confirmed)`
  using `Consumer::LORO`.
- Spawn the run loop on its own tokio task.
- Loop calls `on_inbound_event` per event, then `flush_pending(Consumer::LORO)`
  to write acks.

`event_acks` has zero rows for `loro` across all 1283 events, while `cache`
and `org` are fully caught up. The most likely interpretations (none yet
verified):

1. `LoroSyncController::start` never ran in this binary — the DI graph
   doesn't construct/spawn it for the current GPUI build target.
2. `event_bus.subscribe(filter, Consumer::LORO)` failed silently (no panic
   path was hit because subscribe returns `Result<Receiver, _>` only).
3. The spawned `run_loop` task panicked very early (before processing the
   first event) and the panic was swallowed.

Note that `LoroBackend::create_block` *does* work when called from chord
ops — the global doc and `id_cache` Arc are wired into the cells registry,
so writes succeed and `id_cache` accumulates entries for chord-op-created
blocks. The desync is one-way: SQL → Loro never replays.

## Two fixes

### (1) Primary: fix the inbound runtime

Find why `event_acks.consumer = 'loro'` is empty:

- Confirm `LoroSyncController::start` runs in the GPUI binary (set a
  breakpoint at `loro_module.rs:228`, or add a log + boot the app and
  watch the structured log stream).
- If it doesn't run, the DI registration for `LoroSyncControllerHandle`
  isn't being resolved. Inspect `Injector::resolve` calls for that type
  in the boot path.
- If it does run, check whether `event_bus.subscribe(filter, Consumer::LORO)`
  returns a usable receiver and whether the spawned task is alive (add a
  `tracing::error!` on Drop of the task handle, or check process state).

### (2) Safety net: mirror `apply_create`'s placeholder logic into the chord-op path

`apply_create` (`loro_sync_controller.rs:1094-1110`) creates a placeholder
root when the parent isn't in Loro. The chord-op path doesn't have that
recovery. If we add it, split_block won't fail when the mirror is out of
sync; the split will land cleanly with a placeholder parent that the
inbound runtime can later reconcile.

Concrete change site:
`crates/holon/src/api/loro_backend.rs::create_block` around line 2022:

```rust
let parent_tree_id = match resolve_parent_tree_id(&tree, &id_cache, &parent_id)? {
    Some(tid) => Some(tid),
    None if !parent_id.is_no_parent() && !parent_id.is_sentinel() => {
        // Mirror apply_create's placeholder path — the Loro mirror is
        // behind SQL on this parent; create a placeholder root rather
        // than fail the chord op.
        let placeholder_uri = self.create_placeholder_root(parent_id.id()).await?;
        resolve_parent_tree_id(&tree, &id_cache, &EntityUri::from_raw(&placeholder_uri))?
    }
    None => None,
};
```

The placeholder reconciles later when the inbound runtime catches up (per
the comment at `loro_sync_controller.rs:1105` and the
`merge_placeholder_into_real` machinery, if present).

## Reproduction

Any block whose `block.created` event has `origin = 'org'` and no
`event_acks` row for `consumer = 'loro'`. In the live DB those are the
overwhelming majority — basically every block created via org parsing.

Check before splitting:

```sql
SELECT 1
FROM events e
LEFT JOIN event_acks a ON a.event_id = e.id AND a.consumer = 'loro'
WHERE e.aggregate_id = 'block:<your-parent-id>'
  AND e.event_type = 'block.created'
  AND a.event_id IS NULL;
```

A row means the split will fail.

## Adjacent

- MEMORY.md `phase3_3_step2_scaffolded` — the inbound gate is intentionally
  disabled by default; `Org` origin events are whitelisted past the gate.
  So the gate isn't the problem. The problem is upstream of the gate: the
  controller's run loop never sees these events.
- MEMORY.md `phantom_loro_startup_seed_race` — the seed query skips blocks
  with unacked `block.created` events on the assumption that the inbound
  runtime will pick them up. When the runtime is dead, the seed gap is
  permanent.
