# Handoff: Subscribe to message notifications for active sessions

## Problem

The MCP server (`claude-code-history-mcp`) already generates
`notifications/resources/updated` for `claude-history://sessions/{session_id}/messages`
when a session's JSONL file changes on disk. But Holon never receives these because
it never subscribes to a concrete message URI.

The vtable notification path (`resync_vtable_by_uri` in `McpSyncEngine`) is wired
and working — it can match `claude-history://sessions/xxx/messages` against the
template and refresh `cc_message` via FDW write-through. But the notification never
arrives because the server only sends to subscribed peers.

## Root Cause

In `subscribe_all()`, vtable subscriptions with dynamic params skip the subscribe
call because the concrete URI isn't known at startup:

```
[McpSyncEngine] Vtable 'cc_message_fdw' has dynamic params ["session_id"]
  — relying on broadcast notifications
```

The server doesn't broadcast — it only notifies subscribers for exact URI matches
(`watch.rs:55-57`).

## Solution

Subscribe to concrete session message URIs when a session becomes active (i.e.,
when a matview priming query or watch_query targets `cc_message` for a specific
`session_id`).

### Option A: Subscribe during FDW prime (recommended)

In `MatviewManager::prime_fdw_caches()` or `McpSyncEngine::resync_vtable_by_uri()`,
after successfully refreshing a vtable cache for a specific session, subscribe to
the concrete URI for ongoing notifications.

This requires:
1. `McpSyncEngine` to expose a `subscribe_uri(uri)` method (thin wrapper around
   `peer.subscribe()`)
2. `MatviewManager` or the vtable refresh path to call it after a successful prime
3. A way for `MatviewManager` to reach `McpSyncEngine` (currently it doesn't)

### Option B: Subscribe from the UI watcher

When `watch_ui` or `watch_query` sets up a matview on `cc_message WHERE session_id = X`,
extract the session_id and subscribe to `claude-history://sessions/X/messages`.

This couples the subscription to the UI lifecycle, which is actually correct —
we only want notifications for sessions the user is actively viewing.

### Option C: Change the MCP server to broadcast

Modify `watch.rs` to notify ALL connected peers for ALL affected URIs, not just
subscribed ones. This is simpler but noisier — every JSONL file change would
trigger a notification even if no one cares about that session.

## MCP Server — No Changes Needed

The server (`claude-code-history-mcp`) already:
- Watches `~/.claude/projects/` recursively for file changes
- Maps `{project_id}/{session_id}.jsonl` changes to
  `claude-history://sessions/{session_id}/messages` URIs
- Sends `notifications/resources/updated` to subscribed peers
- Handles `resources/subscribe` and `resources/unsubscribe`

All changes are on the Holon client side.

## Key Files

### MCP Server (reference only, no changes)
- `/Users/martin/Workspaces/ai/claude-code-history-mcp/src/watch.rs` — file watcher,
  `affected_uris()` maps paths to URIs
- `/Users/martin/Workspaces/ai/claude-code-history-mcp/src/server.rs` — `SubscriptionState`,
  subscribe/unsubscribe handlers, `notify()` sends to subscribers

### Holon Client (changes needed)
- `crates/holon-mcp-client/src/mcp_sync_engine.rs` — `subscribe_all()`,
  `resync_vtable_by_uri()`, `McpSyncEngine::peer` (can call `peer.subscribe()`)
- `crates/holon/src/sync/matview_manager.rs` — `prime_fdw_caches()` knows the
  concrete SQL with session_id

## Implementation Sketch (Option A)

### 1. Add `subscribe_to_resource` on `McpSyncEngine`

```rust
pub async fn subscribe_to_resource(&self, uri: &str) -> anyhow::Result<()> {
    self.peer
        .subscribe(SubscribeRequestParam { uri: uri.to_string() })
        .await
        .map_err(|e| anyhow::anyhow!("Subscribe failed for '{uri}': {e}"))?;
    info!("[McpSyncEngine] Subscribed to '{uri}'");
    Ok(())
}
```

### 2. After vtable refresh, subscribe to the concrete URI

In `resync_vtable_by_uri` or a new method called from `prime_fdw_caches`,
reconstruct the concrete URI from the template + extracted params, and subscribe:

```rust
let concrete_uri = expand_uri_template(&sub.uri_template, &params)?;
self.subscribe_to_resource(&concrete_uri).await?;
```

### 3. Wire McpSyncEngine into MatviewManager (or pass subscribe callback)

Options:
- Store `Arc<McpSyncEngine>` in `MatviewManager` (adds a dependency)
- Pass a `Box<dyn Fn(String) -> Future>` subscribe callback at construction
- Have `prime_fdw_caches` return the concrete URIs, let the caller subscribe

The callback approach is cleanest — `MatviewManager` stays generic, the MCP
layer provides the subscribe behavior.

## Testing

1. Start Holon GPUI
2. Watch a `cc_message` query for a specific session → primes cache
3. Check logs for `[McpSyncEngine] Subscribed to 'claude-history://sessions/xxx/messages'`
4. In another Claude Code session, send a message → JSONL file changes
5. Check logs for `[subscription_listener] Resource updated: claude-history://sessions/xxx/messages`
6. Verify `cc_message` cache row count increases
7. If a matview watches that session, verify CDC event arrives
