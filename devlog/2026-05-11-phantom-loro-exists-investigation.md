# Phantom-Loro-exists race — static-analysis handoff

Date: 2026-05-11

## Context

Phase 3.7 (`devlog/2026-05-11-095845-phase3.7-typed-position-event-field.md`)
closed the gen-strategy-mismatch class of Full-PBT failures but left
one structurally distinct flake open. The remaining symptom: in a
5-block BulkExternalAdd batch, **block 1 flows through
`apply_create` normally; blocks 2-5 take the early-bypass to
`apply_update_with_backend` because Loro reports they already exist**.
The typed positional plumbing is correct — what's missing is an
understanding of *which writer* puts blocks 2-5 in Loro before their
CDC arrives.

This devlog captures the static-analysis work done this session, with
explicit "ruled in/out" notes per candidate. Goal: turn the 25-min
PBT reproduction run into a directed investigation rather than
open-ended bisection.

## What "already exists" means concretely

`crates/holon/src/sync/loro_sync_controller.rs::apply_create`:

```rust
if backend.resolve_to_tree_id(block_id).await.is_some() {
    return apply_update_with_backend(backend, data, position_after_block_id).await;
}
```

`resolve_to_tree_id` (`crates/holon/src/api/loro_backend.rs:1560`) first
tries to parse as a `block:peer:counter` TreeID. For bulk-add stable
IDs like `block:bulk-0-0` that fails; the slow path
(`find_tree_id_by_stable_id`) walks every tree node and looks for
`STABLE_ID` meta matching the bare ID.

So the question is: **who has written a tree node with
`STABLE_ID = "bulk-0-N"` before its CDC event hits `on_inbound_event`?**

## STABLE_ID writers — static enumeration

`grep meta.insert.*STABLE_ID` in `crates/holon/src/api/loro_backend.rs`
returns four sites:

| Line | Function | Notes |
|---|---|---|
| 1603 | `set_external_id` | Foreign entity IDs (Todoist). Bare ID is the foreign external_id; STABLE_ID gets `external_id.strip_prefix("block:")`. Wouldn't produce a `bulk-0-N` shape unless a Todoist sync is leaking those IDs — unlikely in PBT. |
| 1620 | `create_placeholder_root` | Root-level placeholder for inbound parent references. The `stable_id` arg is `parent_entity.id()` — the orphan parent's bare id. Could produce a STABLE_ID `bulk-0-N` if some block creation has `parent_id = bulk-0-N` BEFORE block 2's create event arrives. |
| 1980 | `create_block` (singular) | Standard create. Sets STABLE_ID to the `id.id().to_string()` arg. |
| 2170 | `create_blocks` (plural) | Batched create. Only called from `crates/holon/src/api/pbt_infrastructure.rs:231` — used by `loro_backend_pbt` tests, NOT production code paths. |

The interesting candidate is **`create_placeholder_root`**.

## `create_placeholder_root` is invoked from two production paths

Both `LoroSyncController::apply_create` and
`seed_loro_from_persistent_store::apply_seed_row` use the same
pattern: if `parent_id` isn't in Loro yet, create a placeholder root
with `STABLE_ID = parent_id.id()` so the child has a home.

```rust
let parent_uri = if backend.resolve_to_tree_id(parent_id_raw).await.is_some() {
    EntityUri::from_raw(parent_id_raw)
} else {
    let parent_entity = EntityUri::from_raw(parent_id_raw);
    if parent_entity.is_no_parent() || parent_entity.is_sentinel() {
        parent_entity
    } else {
        let placeholder_uri = backend
            .create_placeholder_root(parent_entity.id())
            .await
            .map_err(...)?;
        EntityUri::from_raw(&placeholder_uri)
    }
};
```

**Hypothesis A (placeholder collision):** In a BulkExternalAdd burst,
the first inbound `apply_create` for block 2 finds its parent
(`bulk-0-0` — which IS block 1 created seconds ago, but the cache
lookup loses it). Hits the else branch, calls
`create_placeholder_root("bulk-0-0")`. That sets STABLE_ID on a NEW
root-level node to "bulk-0-0". Now when block 1's create event
arrives next (out of order?), `apply_create` finds the placeholder
node with STABLE_ID="bulk-0-0" and takes the early-bypass.

Two ways this could happen:

1. **Block events arrive out of order.** OrgSyncController emits
   `[create-1, create-2, create-3, create-4, create-5]` in document
   order via `execute_batch_with_origin`. The bus publishes them in
   one transaction. The subscriber's CDC stream delivers them in
   `created_at, id` order. The monotonic ULID generator (Phase 3.7)
   guarantees `id` order matches publish order WITHIN ONE MS. So
   in-order delivery is normally guaranteed.

   But: `parent_id` on block 2 might NOT be `bulk-0` (the first
   sibling) — it'd be the doc URI (their parent). Block 2's parent
   is the doc, not block 1. So `apply_create(block 2)` resolving
   parent finds the doc, not block 1. Placeholder isn't created for
   block 1's id.

2. **`apply_create`'s placeholder creates a STABLE_ID matching the
   block-2 ID itself.** Not possible by the code — placeholder is
   keyed on `parent_id`, never the block's own id.

Hypothesis A as stated doesn't survive close reading. The placeholder
is keyed on parent, not on the block. For blocks 2-5 to all show
"already exists", their *own* IDs (`bulk-0-1` … `bulk-0-4`) must have
landed in the tree as STABLE_IDs. Placeholders are keyed on parents.

**Update:** Hypothesis A is ruled out by code inspection.

## `create_block` (singular) — the only writer that sets STABLE_ID = child's-own-id

`LoroBackend::create_block` (line 1959) sets `STABLE_ID = stable_id`
where `stable_id = id.id().to_string()` — the *target* block's bare id.

Callers in production:

| Caller | Use | In scope for the flake? |
|---|---|---|
| `LoroSyncController::apply_create` | Inbound CDC apply | YES — but only after the bypass check passes. The bypass is the symptom, so apply_create can't be the cause for blocks 2-5. |
| `LoroSyncController::apply_update_with_backend` | NO — doesn't call create_block. | n/a |
| `LoroBlockOperations::create` (`sync/loro_block_operations.rs:280`) | Direct CRUD via this struct. NOT wired as an OperationProvider — see comment in `loro_module.rs:104`. | UNCLEAR — DI registers it, but no production code seems to dispatch to it. |
| `seed_loro_from_persistent_store::apply_seed_row` | Startup seed from SQL | UNLIKELY — runs ONCE at DI factory time, before the controller starts. New BulkExternalAdd blocks aren't in SQL yet at seed time. |

The first interesting unsolved lead is **`LoroBlockOperations::create`**.

## `LoroBlockOperations` — registered in DI but no production dispatcher

`crates/holon/src/sync/loro_module.rs:76-83` registers
`LoroBlockOperations` as a DI provider. The next 4 lines explicitly
state it is NOT registered as `OperationProvider`:

```
// NOTE: LoroBlockOperations is NOT registered as an OperationProvider.
// All block CRUD operations go through SqlOperationProvider → Turso
// (source of truth). Loro is populated via EventBus subscriptions
// (reverse sync), not through the command path.
```

So nobody should call `LoroBlockOperations::create` in production. But
DI still resolves it on demand. `grep '\.resolve.*LoroBlockOperations'`
turns up zero direct callers in `crates/holon/src` and
`crates/holon-orgmode/src`. **Worth confirming with a runtime trace**
(e.g., adding `tracing::error!` at the top of
`LoroBlockOperations::create` and re-running the PBT) before chasing
further.

## Other writers — explicitly ruled out

- **OrgmodeSyncProvider** (`crates/holon-orgmode/src/orgmode_sync_provider.rs`):
  Operates on `Directory` and `File` entities only. Its
  `execute_operation` dispatches to `self.sync()`, which scans the
  filesystem; it does not touch blocks.
- **CacheEventSubscriber**: Writes to `QueryableCache`, not Loro tree.
- **iroh-sync / loro_share_backend** (peer import): These paths exist
  but require an active sharing session. In the PBT's single-process
  setup, no peers are connected, so `doc.import(&peer_delta)` paths
  are dormant.
- **LoroDocument::load_from_file**: Loads the
  `<storage_dir>/holon_tree.loro` snapshot on first
  `get_global_doc()`. The PBT uses `tempfile::tempdir()` for the
  storage dir per test case; no stale snapshot from a prior run
  should bleed across. Worth confirming the temp dir isn't reused.

## Inbound→outbound feedback loop — looked at and shelved

`apply_create` mutates Loro → `doc.subscribe_root` fires → wake
notifies outbound `on_loro_changed` → outbound diff sees the new
block, calls `execute_batch_with_origin([create-block-1],
EventOrigin::Loro)` → SQL `INSERT OR IGNORE` (no-op) + publish_batch
emits a Loro-origin event → that event echos back into the inbound
queue → echo-suppressed (`EventOrigin::Loro` → `EchoSuppress`).

This loop doesn't write to Loro on the inbound side, so it can't
produce phantom STABLE_IDs. Confirmed by reading
`on_inbound_event_inner`'s `InboundEventDecision::EchoSuppress` path
(returns early without touching backend).

## Suggested next steps (in execution order, smallest first)

1. **Confirm `LoroBlockOperations::create` is genuinely dormant in
   production.** Add a `tracing::error!("UNEXPECTED LoroBlockOperations::create");`
   at the function entry and run the PBT once. If the error fires:
   that's the phantom path. If it doesn't: rule it out and move on.
   Single-line change, no risk.

2. **Confirm the temp dir isolation.** Add a one-line
   `tracing::info!` logging `LoroDocumentStore::snapshot_path()` at
   the `if snapshot_path.exists()` check
   (`loro_document_store.rs:75`). Run the PBT once. If any non-empty
   load fires before BulkExternalAdd, that's a leak across cases.

3. **Instrument the slow-path `find_tree_id_by_stable_id` to log the
   matched node's TreeID.** Specifically log
   `(stable_id, tree_id, parent_state)` at the
   `if *sid == stable_id_owned` line in
   `loro_backend.rs:1540`. A TreeID with `(peer, counter)` not
   matching the LoroDocument's current peer would point to a peer
   import; a counter > the highest seen so far would point to a
   batched-create write the test fixture's seed.

4. **As a structural counter-test**, write a focused unit test that
   reproduces the BulkExternalAdd flow at the apply_create level
   without going through the full PBT: seed 5 events into the
   EventBus with origin=Org, wait for `inbound_runtime_applied_count`
   to reach 5, then assert each `apply_create` traced through the
   non-bypass branch. This collapses the 25-min reproduction to
   seconds and lets bisection drive cleanly.

5. **Only if 1-4 don't yield a culprit**, run the actual Full PBT
   with `PROPTEST_CASES=1` + the `[POS-TRACE]` instrumentation
   already mentioned in the Phase 3.7 devlog.

## What's already ruled out

- Phase 3.7's typed-positional plumbing — works correctly. The trace
  shows `(typed)` and the correct parent/after on every apply.
- gen-strategy mismatch in `apply_sort_key_hint` — that path is
  dormant for OrgSync producers (they always emit `after_block_id`).
- `seed_loro_from_persistent_store` — runs once before the controller
  starts, before BulkExternalAdd would populate SQL.
- Outbound echo on origin=Loro events — echo-suppressed, never
  touches Loro on the inbound side.
- CacheEventSubscriber — writes to QueryableCache, not Loro.
- Peer-import paths — dormant without an active iroh session.

## Roadmap status (after this session)

| # | Item | Status |
|---|---|---|
| 1 | Resolve phantom-Loro-exists flake | **OPEN — investigation handoff here** |
| 2 | Flip the gate | blocked on (1) |
| 3 | Gate integration test | blocked on (2) |
| 4 | Retire sort_key write path entirely | blocked on (2) |
| 5 | Typed `_routing_doc_uri` | LANDED |
| 6 | Chord-op direct positioning via cell registry | blocked on (4) |
| 7 | archlint rule for new `_routing_*` payload keys | LANDED |

Items 5 and 7 landed this session. Item 1 has a directed plan above.
