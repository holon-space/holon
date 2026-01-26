# Item 4 phases 1 + 2 + 3a + 4 — typed `write_position`; `BlockOrdering` trait encapsulates the SqlOnly fallback

Date: 2026-05-11

## Outcome

Three phases of item 4 ("retire sort_key write path entirely, Stage 2-plus")
landed this session. The chord-op hot paths for `move_block`, `indent`,
`outdent`, and `join_block`'s child re-parent loop now route through a typed
positioned-move API in Loro mode — no `gen_key_between` string round-trip.
SqlOnly mode keeps the legacy `set_field("sort_key", gen_key_between(...))`
shape as a documented fallback. Two full PBT runs green (553s, 545s).

## Changes

### Phase 1 — `EntityCellRegistry::write_position` (typed positional intent)

`crates/holon-core/src/cell_registry.rs`: trait now `#[async_trait]`; new
default method

```rust
async fn write_position(
    &self,
    _: &EntityUri,
    _: &str,
    _: Option<&str>,
) -> Result<bool> {
    Ok(false)
}
```

`Ok(true)` ⇒ the registry executed the move (Loro-backed). `Ok(false)` ⇒
this registry can't satisfy positional intent here (SqlOnly mode, no
backing tree). Default returns `Ok(false)` so non-block registries opt out
for free.

`crates/holon/src/sync/block_cell_registry.rs`: override on
`BlockCellRegistry` calls `LoroBackend::update_block_position(target, parent,
predecessor)` directly. SqlOnly mode returns `Ok(false)`.

### Phase 2 — `BlockOperations::move_to_position` uses `write_position`

`crates/holon-core/src/traits.rs::move_to_position`: in Loro-backed mode,
`cells().write_position(uri, parent, after_id)` runs the typed move directly.
No `gen_key_between` computation, no `set_field("sort_key", ...)` emission,
no registry sibling-scan. In SqlOnly mode (or any registry returning
`Ok(false)`), falls back to the legacy compute + paired set_field path so
the SQL `block.sort_key` column carries the fractional-index value
verbatim — same contract as before.

This change covers `move_block` (line 657 in traits.rs) and `indent` /
`outdent` (which both delegate to `move_block`).

### Phase 3a — `join_block`'s child re-parent loop

`crates/holon-core/src/traits.rs::join_block` (~line 994): the inner loop
threading `last_sort_key` across iterations is gone. Replaced with
`last_after_id` (a stable block id) and a single `move_to_position(child,
target, last_after_id)` call per child. Same SQL-projection-race avoidance
(loop tracks state locally), but now via stable block ids — no gen_key_between
in the loop body in Loro mode.

### Phase 4 — `BlockOrdering` trait encapsulates the (Loro vs SqlOnly) split

`crates/holon-core/src/block_ordering.rs`: new trait

```rust
#[async_trait]
pub trait BlockOrdering: Send + Sync {
    async fn place(&self, uri: &EntityUri, parent_id: &str, after_id: Option<&str>) -> Result<()>;
    async fn new_child_anchor(&self, parent_id: &str, after_id: Option<&str>) -> Result<String>;
}
```

`BlockOperations` trait gained a `fn ordering(&self) -> Option<&dyn BlockOrdering>`
accessor (default `None`; production impls must override).

Deleted from `crates/holon-core/src/traits.rs`:

- `compute_sort_key_between_neighbors` (the gen_key_between + sibling scan
  helper). Gone — `holon-core::traits` no longer imports `gen_key_between`.
- The SqlOnly fallback branch of `move_to_position` (gen_key_between + paired
  `set_field`). `move_to_position` is now a thin three-line delegation:
  `self.ordering()?.place(uri, parent, after_id).await?; Ok(Vec::new())`.
- `split_block::fallback_sort_key`. Replaced with a single
  `ordering().new_child_anchor(parent, Some(id))` call — chord-op code no
  longer touches gen_key_between.

`crates/holon/src/core/sql_block_operations.rs`: `SqlBlockOperations`
implements `BlockOrdering` directly (`ordering()` returns `Some(self)`).
The impl encapsulates the entire conditional:

- `place`: tries `cell_registry.write_position` (Loro mode, succeeds in one
  call to `tree.mov_after`). If `Ok(false)`, runs `new_child_anchor` then
  emits the paired `set_field` writes via `sql_ops.execute_operation`.
- `new_child_anchor`: scans `cache.get_all().await?` for neighbors and runs
  `gen_key_between`. Loro mode silently overwrites the result via
  `apply_create` reading `Event::position_after_block_id` — the unused
  compute is the price of having one code path.

`crates/holon-core/src/block_operations_tests.rs`: the synthetic `MemStore`
test substrate gets a matching `BlockOrdering` impl (held on the same
struct, mirrors the SqlBlockOperations pattern).

## Verification

| run | what | result |
|---|---|---|
| 1 | phase 1 + phase 2 (move_to_position via write_position) | full PBT 2/2 pass, 553s |
| 2 | phase 3a (join_block uses move_to_position) | full PBT 2/2 pass, 545s |
| 3 | phase 4 (BlockOrdering trait; legacy code deleted) | full PBT 2/2 pass, 526s |

Plus: `cargo test -p holon-core --lib` 47/47; `cargo test -p holon --lib
sync::loro_sync_controller` 16/16; `cargo test -p holon-integration-tests
--test inbound_runtime_gate` 3/3; `cargo test -p holon-integration-tests
--test phantom_loro_exists_repro` 2/2.

## What's still open in item 4

- `gen_key_between` now imports only into `holon-core::fractional_index`
  (its definition site), `holon-core::block_operations_tests`
  (MemStore's BlockOrdering impl), and `holon::core::sql_block_operations`
  (SqlBlockOperations' BlockOrdering impl). Chord-op call sites in
  `holon-core::traits` are gen_key_between-free.
- `BlockCellRegistry::compute_position_for_sort_key` + the registry's
  `"sort_key"` arm in `write_field` are still reachable from the inbound CDC
  path (`apply_sort_key_hint`). As long as non-Loro origins (Org, Todoist,
  external peers) ship sort_key strings, this arm earns its keep. Could be
  retired when all inbound CDC writers are migrated to typed positional
  intents — that's a Phase-3.x-plus discussion.
- Read-side leak: `Block::sort_key()` is still observable to readers (Org
  renderer, event payloads, SQL ORDER BY). Hiding it behind
  `BlockOrdering::siblings_ordered(parent)` is a separate, larger refactor
  worth doing only if a third backing (e.g. external graph DB) materialises.

## Files changed

- `crates/holon-core/src/cell_registry.rs` — `async_trait` on the trait;
  default `write_position`.
- `crates/holon-core/src/block_ordering.rs` — NEW. `BlockOrdering` trait.
- `crates/holon-core/src/lib.rs` — `pub mod block_ordering`.
- `crates/holon-core/src/traits.rs` — `move_to_position` becomes a thin
  delegation to `ordering().place`; `compute_sort_key_between_neighbors`
  deleted; `split_block::fallback_sort_key` replaced with
  `ordering().new_child_anchor`; `gen_key_between` import gone;
  `join_block`'s child loop uses `move_to_position`.
- `crates/holon-core/src/block_operations_tests.rs` — MemStore impls
  `BlockOrdering`.
- `crates/holon/src/sync/block_cell_registry.rs` — `#[async_trait]` impl;
  `write_position` override calling `update_block_position`.
- `crates/holon/src/core/sql_block_operations.rs` — `SqlBlockOperations`
  impls `BlockOrdering` (place + new_child_anchor), `ordering()` returns
  `Some(self)`.
- `devlog/2026-05-11-item4-phase1-3a-write-position.md` — this file.
