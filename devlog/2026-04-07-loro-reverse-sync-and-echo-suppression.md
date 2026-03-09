# Handoff: Loro Reverse Sync Fix + Echo Suppression + Multi-Peer Sync

## What was done

### 1. Echo suppression race fix (`org_sync_controller.rs`)
`on_file_changed` no longer re-renders or writes back to disk. Sets `last_projection = disk_content` instead. This eliminates the race where re-render from batch #2 overwrote batch #3's file write. Also removed the CDC wait loop (no longer needed).

**Files**: `crates/holon-orgmode/src/org_sync_controller.rs`, `crates/holon-orgmode/src/di.rs`

### 2. Real MCP test server (`pbt_mcp_fake.rs`)
Replaced `PbtMcpFake` (direct DB write) with `PbtMcpIntegration` using `tokio::io::duplex` + rmcp. Exercises: server → client handshake → `McpSyncEngine` → `QueryableCache` → Turso IVM.

Key: `tokio::try_join!` for concurrent handshake (sequential blocks), seed-before-subscribe ordering.

**Files**: `crates/holon-integration-tests/src/pbt_mcp_fake.rs`, `crates/holon-integration-tests/src/pbt/sut.rs`, `crates/holon-integration-tests/Cargo.toml`, `crates/holon-mcp-client/src/{lib.rs,mcp_integration.rs}`

### 3. Loro reverse sync: SQL seed at startup (`loro_reverse_sync.rs`)
CDC only delivers changes after matview creation. Blocks ingested during OrgMode file scan were missed. Fix: `seed_from_sql()` queries the block table and creates Loro nodes BEFORE subscribing to CDC. No overlap possible.

**Files**: `crates/holon/src/sync/loro_reverse_sync.rs`, `crates/holon/src/sync/loro_module.rs`

### 4. Loro STABLE_ID metadata (`loro_backend.rs`)
`set_external_id()` and `create_placeholder_root()` now set `STABLE_ID` (stripped of `block:` prefix via `strip_prefix`). `apply_create` uses `EntityUri::from_raw` instead of `EntityUri::block` to avoid double-prefixing.

**Files**: `crates/holon/src/api/loro_backend.rs`

### 5. FieldsChanged property propagation (`loro_reverse_sync.rs`)
Two fixes:
- `apply_fields_changed`: Parses the array-of-tuples format `[["field", old, new]]` that `SqlOperationProvider` emits (was expecting an object map).
- `apply_properties_from_json`: Handles both `Value::Object` (CDC path) and `Value::String` (SQL query path) for the `properties` field.

**Files**: `crates/holon/src/sync/loro_reverse_sync.rs`

### 6. Multi-peer sync infrastructure (`multi_peer.rs`, `peer_ops.rs`)
New `SyncBackend` trait + `DirectSync` implementation for deterministic Loro-to-Loro sync. `multi_peer.rs` (757 lines) provides `PeerState`, `GroupState`, `GroupTransition`, and helpers for generating, applying, and checking multi-peer Loro sync scenarios. Used by both `sync_pbt` (unit-level) and `general_e2e_pbt` (integration).

`peer_ops.rs` wraps `multi_peer` helpers with UUID-based block identity (`PeerBlock` struct) so that peer-created blocks carry the same stable IDs as the primary instance.

PBT transitions added: `AddPeer`, `PeerEdit`, `SyncWithPeer`, `MergeFromPeer`.

**Files**: `crates/holon/src/sync/multi_peer.rs` (new), `crates/holon/src/sync/mod.rs`, `crates/holon-integration-tests/src/pbt/peer_ops.rs` (new), `crates/holon-integration-tests/src/pbt/mod.rs`, `crates/holon-integration-tests/src/pbt/transitions.rs`, `crates/holon-integration-tests/src/pbt/state_machine.rs`, `crates/holon-integration-tests/src/pbt/reference_state.rs`, `crates/holon-integration-tests/src/pbt/transition_budgets.rs`

### 7. `sync_import_with_cdc` bridge (`loro_block_operations.rs`)
`doc.import()` doesn't trigger Loro subscriptions. New method snapshots block state before import, diffs after, and emits synthetic CDC events so the SQL cache stays in sync.

**Files**: `crates/holon/src/sync/loro_block_operations.rs`

### 8. `sync_pbt.rs` extraction → `multi_peer.rs`
Moved ~655 lines of multi-peer PBT infrastructure from `sync_pbt.rs` into the reusable `multi_peer.rs` module. `sync_pbt.rs` now delegates to `multi_peer`.

**Files**: `crates/holon/src/api/sync_pbt.rs`, `crates/holon/src/sync/multi_peer.rs`

### 9. LoroSut assertion improvements (`loro_sut.rs`)
- `build_id_map`: Falls back to using `STABLE_ID` directly when `external_id` TreeID lookup fails.
- Retry loop: Polls on content mismatch (not just count mismatch).
- Filters: Document blocks and placeholder roots excluded from both Loro and Ref sides.

**Files**: `crates/holon-integration-tests/src/pbt/loro_sut.rs`

### 10. Frontend cleanup: remove old reactive types
Simplified `shared_render_entity_build` — removed content_type/source_language/content extraction and match arms. `RenderBlockResult` no longer carries the widget type parameter. Removed unused `render_entity` shadow builders from Dioxus, TUI, and WaterUI frontends. GPUI gets new `drawer.rs` builder and updated `block_ref`/`collection_view` for multi-peer data.

**Files**: `crates/holon-frontend/src/{reactive.rs, reactive_view_model.rs, render_interpreter.rs, shadow_builders/columns.rs, shadow_builders/render_entity.rs}`, `frontends/gpui/src/{render/builders/block_ref.rs, render/builders/drawer.rs, render/builders/mod.rs, views/block_ref_view.rs, views/collection_view.rs, views/render_entity_view.rs, entity_view_registry.rs, geometry.rs, lib.rs, main.rs, examples/design_gallery.rs}`, `frontends/dioxus/src/render/builders/render_entity.rs`, `frontends/tui/src/render/builders/render_entity.rs`, `frontends/waterui/src/render/builders/mod.rs`

### 11. Other fixes
- `phased.rs`: `|_| {}` → `|_| None` (callback return type mismatch)
- `transition_budgets.rs`: Updated EmitMcpData budget for real MCP pipeline costs; added AddPeer/PeerEdit/SyncWithPeer/MergeFromPeer budgets
- `turso_event_bus.rs`: Added `parse_event_row()` with payload Value::Object→String normalization
- `sql_operation_provider.rs`: Fixed `&params` borrow (parallel session artifact)
- `ui_watcher.rs`: Simplified watcher lifecycle (~80 lines changed)
- `test_environment.rs`: Minor adjustments for multi-peer support
- `watch_ui.rs`: Updated test for new APIs

## Current state

All Loro assertions pass. AddPeer budget is in place. The worktree is clean (no unstaged changes).

## Files changed (53 files, +3187 / -1335)

| File | Change |
|------|--------|
| **Core — Loro sync** | |
| `crates/holon/src/sync/loro_reverse_sync.rs` | seed_from_sql, FieldsChanged tuple parser, properties Object/String |
| `crates/holon/src/sync/loro_module.rs` | Pass DbHandle to LoroReverseSyncAdapter |
| `crates/holon/src/sync/loro_block_operations.rs` | New: `sync_import_with_cdc` bridge |
| `crates/holon/src/sync/multi_peer.rs` | New: SyncBackend, PeerState, GroupState, GroupTransition (757 lines) |
| `crates/holon/src/sync/mod.rs` | Re-export multi_peer, loro_block_operations |
| `crates/holon/src/sync/turso_event_bus.rs` | parse_event_row with payload normalization |
| **Core — API** | |
| `crates/holon/src/api/loro_backend.rs` | STABLE_ID in set_external_id + create_placeholder_root, strip_prefix (+419/-) |
| `crates/holon/src/api/sync_pbt.rs` | Extracted to multi_peer.rs (-655 lines) |
| `crates/holon/src/api/ui_watcher.rs` | Simplified watcher lifecycle |
| `crates/holon/src/core/sql_operation_provider.rs` | &params borrow fix |
| **OrgMode** | |
| `crates/holon-orgmode/src/org_sync_controller.rs` | Removed re-render + CDC wait from `on_file_changed` |
| `crates/holon-orgmode/src/di.rs` | DI wiring updates |
| **MCP client** | |
| `crates/holon-mcp-client/src/lib.rs` | Re-export spawn_subscription_listener |
| `crates/holon-mcp-client/src/mcp_integration.rs` | Made spawn_subscription_listener pub |
| **PBT infrastructure** | |
| `crates/holon-integration-tests/src/pbt_mcp_fake.rs` | Replaced PbtMcpFake with real MCP server via duplex |
| `crates/holon-integration-tests/src/pbt/sut.rs` | Rewired to PbtMcpIntegration + multi-peer support |
| `crates/holon-integration-tests/src/pbt/peer_ops.rs` | New: stable-ID-aware peer operations |
| `crates/holon-integration-tests/src/pbt/transitions.rs` | AddPeer, PeerEdit, SyncWithPeer, MergeFromPeer variants |
| `crates/holon-integration-tests/src/pbt/state_machine.rs` | Multi-peer transition generation + application (+314 lines) |
| `crates/holon-integration-tests/src/pbt/reference_state.rs` | Peer tracking in reference model |
| `crates/holon-integration-tests/src/pbt/transition_budgets.rs` | All peer transition SQL budgets (+224 lines) |
| `crates/holon-integration-tests/src/pbt/loro_sut.rs` | build_id_map fallback, retry, document filtering |
| `crates/holon-integration-tests/src/pbt/phased.rs` | `\|_\| None` fix |
| `crates/holon-integration-tests/src/pbt/mod.rs` | Re-export peer_ops |
| `crates/holon-integration-tests/src/test_environment.rs` | Multi-peer adjustments |
| `crates/holon-integration-tests/tests/watch_ui.rs` | Updated for new APIs |
| `crates/holon-integration-tests/tests/general_e2e_pbt.proptest-regressions` | New regression seed |
| `crates/holon-integration-tests/Cargo.toml` | Added rmcp, holon-mcp-client, tracing deps |
| **Frontend — shared** | |
| `crates/holon-frontend/src/render_interpreter.rs` | Simplified shared_render_entity_build |
| `crates/holon-frontend/src/reactive.rs` | Removed old reactive types |
| `crates/holon-frontend/src/reactive_view_model.rs` | Simplified reactive view model |
| `crates/holon-frontend/src/shadow_builders/columns.rs` | Minor cleanup |
| `crates/holon-frontend/src/shadow_builders/render_entity.rs` | Removed unused builder |
| **Frontend — GPUI** | |
| `frontends/gpui/src/render/builders/block_ref.rs` | Multi-peer block_ref updates |
| `frontends/gpui/src/render/builders/drawer.rs` | New: drawer builder |
| `frontends/gpui/src/render/builders/mod.rs` | Register drawer builder |
| `frontends/gpui/src/views/block_ref_view.rs` | Updated for new API |
| `frontends/gpui/src/views/collection_view.rs` | Multi-peer collection support |
| `frontends/gpui/src/views/render_entity_view.rs` | Minor cleanup |
| `frontends/gpui/src/entity_view_registry.rs` | Registry update |
| `frontends/gpui/src/geometry.rs` | Layout adjustments |
| `frontends/gpui/src/lib.rs` | Module exports |
| `frontends/gpui/src/main.rs` | Startup wiring |
| `frontends/gpui/examples/design_gallery.rs` | Gallery additions |
| **Frontend — other (cleanup)** | |
| `frontends/dioxus/src/render/builders/render_entity.rs` | Removed unused render_entity builder |
| `frontends/tui/src/render/builders/render_entity.rs` | Removed unused render_entity builder |
| `frontends/waterui/src/render/builders/mod.rs` | Removed unused builder registration |
| **Other** | |
| `CLAUDE.md` | Updated project instructions |
| `Cargo.lock` | Dependency updates |
