# Virtual Child Feature — Status & Open Questions

## What exists (compiles, all wired except last mile)

| Component | File | Lines |
|-----------|------|-------|
| `VirtualChildSlot` struct | `crates/holon-frontend/src/reactive_view.rs` | 86–89 |
| `VirtualChildRowProvider` impl | `crates/holon-frontend/src/reactive_view.rs` | 97–165 |
| Wraps data source in streaming/snapshot paths | `crates/holon-frontend/src/reactive_view.rs` | 619–622 |
| `VirtualChildConfig` + `virtual_child` field on `ParsedProfile` | `crates/holon/src/entity_profile.rs` | 107–156 |
| `virtual_child_config()` on `BuilderServices` | `crates/holon-frontend/src/reactive.rs` | 58–62, 1525–1532 |
| `virtual_child_slot_from_arg()` — reads `virtual_parent` string arg | `crates/holon-frontend/src/shadow_builders/prelude.rs` | 14–25 |
| `interpret_virtual_child()` — static path helper | `crates/holon-frontend/src/shadow_builders/prelude.rs` | 52–60 |
| `resolve_virtual_parent()` — replaces `true` sentinel with context_id | `crates/holon-frontend/src/render_interpreter.rs` | 596–623 |
| Called in live_query path only | `crates/holon-frontend/src/render_interpreter.rs` | 574 |
| tree/outline/list builders pass `virtual_child` through | `crates/holon-frontend/src/shadow_builders/{tree,outline,list}.rs` | ~15–25 each |
| `block_profile.yaml` declares `virtual_child` defaults | `assets/default/types/block_profile.yaml` | 71–74 |

## What is missing (the last mile)

### Gap 1 — YAML doesn't opt in
`collection_profile.yaml` (`assets/default/types/collection_profile.yaml`, lines 25–27) has:
```
render: 'tree(#{parent_id: col("parent_id"), sortkey: col("sequence"), item_template: render_entity()})'
```
`virtual_parent: true` is absent. The sentinel must be written in YAML by the DSL author so that
only explicitly editable collections get a virtual child (non-editable collections stay clean).

### Gap 2 — live_block path never calls the resolver
`resolve_virtual_parent` is only called inside `shared_live_query_build`
(`render_interpreter.rs:574`) — that is, for `live_query(#{item_template: tree(...)})` DSL.

For the **live_block path** (`block_domain.rs::collection_render_from_profile`, lines 119–188),
the render_expr from the collection profile is returned as-is. `virtual_parent: true` (a `Bool`)
would survive unresolved into the tree builder, where `virtual_child_slot_from_arg` calls
`get_string("virtual_parent")` → `None` (Bool ≠ String).

Resolution must happen somewhere between profile → Structure event. Options (to discuss):

**Option A — substitute in `block_domain.rs`**
In `render_entity` / `collection_render_from_profile` (lines 108–188), after getting the
variant render_expr, walk it and replace `virtual_parent: true` with
`virtual_parent: entity_uri.to_string()`. Same logic as `resolve_virtual_parent` but
lives in `holon` crate (not `holon-frontend`), so no cross-crate dep issue.

**Option B — substitute in `reactive.rs::watch_live`**
In `watch_live` (line ~1092), before calling `services.interpret(&expr, &ctx)`, apply
the substitution using the known `block_id`. `resolve_virtual_parent` would need to be
made `pub` or moved to `holon-api`.

**Option C — make `virtual_child_slot_from_arg` handle `Bool(true)`**
Instead of a sentinel replacement pass, teach `virtual_child_slot_from_arg` to also
accept `Bool(true)` and look up the parent from `ba.ctx.row().get("id")`. Works only if
the row context carries the block's own `id`, which it does when the `live_block`
re-interprets for a fresh Structure event.

## Crate dependency constraint
```
holon-frontend  →  holon  →  holon-api
```
`resolve_virtual_parent` (holon-frontend) cannot be called from `block_domain.rs` (holon).
Options A and C avoid this; Option B requires making the function pub and moving it to holon-api.

## Next step after wiring: materialization
When user types into the virtual row, `ViewEventHandler::handle_text_sync()`
(`crates/holon-frontend/src/view_event_handler.rs`) must detect `id` starting with `virtual:`
and create a real block instead of updating a non-existent one.
