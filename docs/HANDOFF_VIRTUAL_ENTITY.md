# Handoff: Virtual Entity Feature

**Date**: 2026-04-24
**Status**: Infrastructure complete, end-to-end not yet wired

## Goal

When viewing a block's children in a collection (tree/list/outline), always show a virtual editable block at the end. Typing into it materializes a real block. Like LogSeq's always-present empty block.

## What Was Built

### 1. Entity Profile Extension (`entity_profile.rs`)

- **Merged `RawEntityProfile` into `ParsedProfile`** — eliminated struct duplication. `ParsedProfile` now derives `Deserialize` and is the single serde target + public API.
- **Added `VirtualChildConfig`** on `EntityProfile`:
  ```rust
  pub struct VirtualChildConfig {
      pub defaults: HashMap<String, Value>,  // field defaults for the virtual DataRow
  }
  ```
- `ParsedProfile` and `EntityProfile` both carry `virtual_child: Option<VirtualChildConfig>`
- **`ProfileResolving` trait** gained `virtual_child_config(&self, entity_name: &str) -> Option<VirtualChildConfig>` with default `None`; implemented on `ProfileResolver` via cache lookup.
- **`block_profile.yaml`** declares:
  ```yaml
  virtual_child:
    defaults:
      content: ""
      content_type: "text"
  ```

### 2. `render_entity()` as Default Item Template

**Key insight**: `block_ref()` was causing N+1 queries — each block in a collection spawned a separate watcher that re-fetched data the collection already had. `render_entity()` renders from the current row data via profile resolution. No DB fetch.

- `block_ref()` is still needed ONLY for `query_block` variant (blocks with embedded queries that need their own watcher)
- With `render_entity()`, the profile's `query_block` variant renders as `block_ref()` — so query blocks still get watchers, but leaf blocks render inline

**Changed**:
- `crates/holon-integration-tests/src/pbt/reference_state.rs` — `valid_render_expressions()` and `default_root_render_expr()` use `render_entity()` instead of `block_ref()`
- `crates/holon/src/api/block_domain.rs` — `render_leaf_block()` uses `render_entity()` instead of `render_entity()`

**NOT changed** (user data):
- Existing org files / DB entries that use `block_ref()` as item_template continue to work. The `block_ref` widget is preserved.

### 3. `VirtualChildRowProvider` (`reactive_view.rs`)

Wraps any `ReactiveRowProvider` and appends a virtual DataRow at the end using `SignalVecExt::chain(always(vec![virtual_row]))`:

```rust
struct VirtualChildRowProvider {
    inner: Arc<dyn ReactiveRowProvider>,
    virtual_row: Arc<DataRow>,       // synthetic row from defaults
    virtual_key: String,             // "block:virtual:{parent_id}"
}
```

The virtual row contains:
- `id`: `"block:virtual:{parent_id}"`
- `parent_id`: the context block's ID
- `sort_key`: `f64::MAX` (sorts last in trees)
- All defaults from `VirtualChildConfig` (e.g., `content: ""`, `content_type: "text"`)

The wrapping happens in `ReactiveView::create_driver()` — ONE injection point. Drivers and collection widgets are completely unaware.

### 4. Wiring Path

```
live_query builder (render_interpreter.rs)
  → looks up context_id's entity profile via services.virtual_child_config()
  → creates VirtualChildSlot { defaults, parent_id }
  → sets RenderContext.virtual_child

collection builder (tree.rs / list.rs / table.rs / outline.rs)
  → passes ba.ctx.virtual_child to streaming_collection()
  → stored in CollectionConfig.virtual_child

ReactiveView::create_driver()
  → if virtual_child is Some, wraps data_source in VirtualChildRowProvider
  → driver sees virtual row as just another data row
  → render_entity() resolves block profile → renders as editable_text
```

### 5. `BuilderServices` Extension

- Added `virtual_child_config(&self, entity_name: &str) -> Option<VirtualChildConfig>` to the trait (default `None`)
- Implemented on `ReactiveEngine`'s `BuilderServices` impl — delegates to `session.engine().profile_resolver().virtual_child_config()`

## What's NOT Done Yet

### A. The Virtual Child Doesn't Appear Yet

The `live_query` builder sets `RenderContext.virtual_child` during **synchronous interpretation** (the initial empty build in `shared_live_query_build`). But in the **GPUI streaming path**, the content is re-interpreted asynchronously when the `ReactiveShell` processes the `LiveQuery` slot. Need to verify that `virtual_child` survives into the re-interpretation context.

**Where to look**: `frontends/gpui/src/views/reactive_shell.rs` (or wherever the LiveQuery slot's content gets re-interpreted with the actual `data_source`). The `RenderContext` used for that re-interpretation must carry the `virtual_child` from the initial build.

### B. Materialization (Virtual → Real Block)

When the user types into the virtual entity's `editable_text` and blurs:
1. `ViewEventHandler::handle_text_sync()` fires with `id = "block:virtual:{parent_id}"`
2. Currently it dispatches `set_field` which will fail (no such block in DB)

**What needs to happen**:
- Detect the `block:virtual:` prefix in the entity ID
- Instead of `set_field`, dispatch a `create_block` operation (parent_id extracted from the virtual ID, content from the text)
- After creation, the CDC delivers the real block → collection updates → a new virtual row appears at the end

**Implementation options**:
- Add `create_child_block` operation to `BlockOperations` trait (cleanest)
- Or call `backend.create_block()` directly via a new `dispatch_intent` variant
- The `ViewEventHandler` is at `crates/holon-frontend/src/view_event_handler.rs:128`

### C. PBT Support

The PBT reference state (`HeadlessBuilderServices`) returns `None` for `virtual_child_config()` (default trait impl). If we want PBTs to exercise virtual entities:
- Implement `virtual_child_config` on `HeadlessBuilderServices` using the test profile's entity name
- Add a virtual entity edit transition to the PBT state machine

### D. Focus Behavior

When the user navigates to a block with children, the virtual child should be focusable. Need to verify:
- `FocusPath` can find the virtual entity in the tree
- Navigation (arrow keys) can reach it
- Clicking on it activates the `editing` profile variant (via `is_focused` condition)

## Files Modified (This Session)

| File | Change |
|------|--------|
| `crates/holon/src/entity_profile.rs` | Merged `RawEntityProfile`→`ParsedProfile`, added `VirtualChildConfig`, `ProfileResolving::virtual_child_config()` |
| `assets/default/types/block_profile.yaml` | Added `virtual_child: { defaults: { content: "", content_type: "text" } }` |
| `crates/holon-frontend/src/reactive_view.rs` | Added `VirtualChildSlot`, `VirtualChildRowProvider`, `CollectionConfig.virtual_child`, wrapping in `create_driver()` |
| `crates/holon-frontend/src/reactive_view_model.rs` | `streaming_collection()` takes `virtual_child` param |
| `crates/holon-frontend/src/render_context.rs` | Added `virtual_child: Option<VirtualChildSlot>` |
| `crates/holon-frontend/src/render_interpreter.rs` | `shared_live_query_build()` looks up virtual_child from profile, sets on context |
| `crates/holon-frontend/src/reactive.rs` | `BuilderServices::virtual_child_config()` trait method + impl |
| `crates/holon/src/type_registry.rs` | `apply_parsed_profile()` updated for merged `ParsedProfile` |
| `crates/holon/src/api/block_domain.rs` | `render_leaf_block()`: `render_entity` → `render_entity` |
| `crates/holon-integration-tests/src/pbt/reference_state.rs` | `block_ref` → `render_entity` in item templates |
| `crates/holon-frontend/src/shadow_builders/{tree,list,table,outline,columns}.rs` | Pass `virtual_child` to `streaming_collection()` |

## Architecture Diagram

```
EntityProfile.virtual_child (YAML defaults)
        │
        ▼
live_query builder ─── services.virtual_child_config("block") ──► ProfileResolver cache
        │
        ▼
RenderContext.virtual_child = VirtualChildSlot { defaults, parent_id }
        │
        ▼
collection builder (tree/list/...) ──► streaming_collection(virtual_child)
        │
        ▼
CollectionConfig.virtual_child ──► ReactiveView::new_collection()
        │
        ▼
create_driver() wraps data_source in VirtualChildRowProvider
        │
        ▼
VirtualChildRowProvider.chain(always(vec![virtual_row]))
        │
        ▼
Driver sees virtual row as normal data ──► render_entity() ──► block profile ──► editable_text
```

## Key Design Decisions

1. **Virtual child = DataRow, not ViewModel** — goes through normal profile resolution, works for any entity type
2. **`render_entity()` replaces `block_ref()` as default** — eliminates N+1 query per collection item; `block_ref()` only where actually needed (query blocks)
3. **`chain()` for signal composition** — `futures_signals::SignalVecExt::chain` cleanly appends the virtual row without custom `SignalVec` impls
4. **One injection point** — `create_driver()` is the only place that knows about virtual children; drivers and collection widgets are unmodified
5. **Profile declares defaults, not render expression** — the virtual child renders through the same profile pipeline as real entities; no special rendering code
