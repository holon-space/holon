# Browser MCP Relay — Review Hand-off

**Branch**: `browser-mcp-relay` worktree  
**Date**: 2026-04-15  
**Status**: Implementation complete; E2E tested with mock browser. Real-browser smoke test pending.

---

## What Was Built

Claude Code currently has MCP tool access to the GPUI frontend via `holon-direct`, which talks
to a native `BackendEngine`. When the Dioxus web frontend is active, the authoritative engine
state lives inside a `holon-worker` Web Worker (WASM). This feature lets Claude Code call the
same MCP tools against the in-browser engine, with no duplication of tool schemas.

---

## Architecture

```
Claude Code
  → http://localhost:8766/mcp          (single port, serve.mjs proxies)
     → holon-mcp process (RELAY_PORT=3002)
        BrowserRelayServer.list_tools  → tool schemas from Rust #[tool] annotations
        BrowserRelayServer.call_tool   → BrowserRelay.forward(req)
           → WebSocket (tokio-tungstenite) → ws://localhost:8766/mcp-hub?role=native
              ↕ serve.mjs hub bridges native ↔ browser
           ← WebSocket (web_sys) ← Dioxus page ?role=browser
              → bridge.call("engineMcpTool", [tool, args_json])
                 → holon-worker WASM engine_mcp_tool(name, args_json) → JSON
```

**Key invariant**: tool schemas stay exclusively in `frontends/mcp/src/tools.rs` (`#[tool]`
annotations). `BrowserRelayServer` calls `list_all()` on the existing tool routers; it never
duplicates schema definitions.

---

## Wire Protocol

```jsonc
// native → browser (tool call request)
{ "id": "uuid", "tool": "execute_query", "arguments": {"sql": "...", "language": "sql"} }

// browser → native (success)
{ "id": "uuid", "content": "[{\"type\":\"text\",\"text\":\"...\"}]" }

// browser → native (error)
{ "id": "uuid", "is_error": true, "content": "[{\"type\":\"text\",\"text\":\"...\"}]" }
```

Hub routing: `?role=native` (Rust relay) or `?role=browser` (Dioxus page).

---

## Files Changed / Added (relay-specific)

### New

| File | What it does |
|------|-------------|
| `frontends/mcp/src/browser_relay.rs` | `BrowserRelay` — WS client to hub, pending map, reconnect loop. `BrowserRelayServer` — `ServerHandler` impl: `list_tools` from Rust annotations, `call_tool` forwarded to relay. |
| `frontends/dioxus-web/serve.mjs` | Dev server: serves `dist/`, `/web/`, WASM. **New**: WS hub at `/mcp-hub`, HTTP proxy at `/mcp` → `RELAY_PORT`. |

### Modified

| File | What changed |
|------|-------------|
| `frontends/mcp/src/di.rs` | `run_http_server` checks `HOLON_BROWSER_RELAY_URL` env var first; if set, runs `BrowserRelayServer` instead of `HolonMcpServer`. |
| `frontends/mcp/src/main.rs` | Relay mode: reads `HOLON_BROWSER_RELAY_URL`; if set, skips DB/engine init, binds on `RELAY_PORT`, runs relay-only server. |
| `frontends/mcp/src/lib.rs` | `pub mod browser_relay;` |
| `frontends/mcp/Cargo.toml` | Added `tokio-tungstenite = "0.26"`, `uuid = { features = ["v4"] }`. |
| `frontends/dioxus-web/src/main.rs` | Added `connect_mcp_relay(bridge)` call after `engineInit`. Added `MCP_WS` thread-local. `connect_mcp_relay` fn: opens WS as `role=browser`, handles `onmessage` (dispatch to `engineMcpTool`), `onclose` (reconnect loop). |
| `frontends/holon-worker/src/lib.rs` | Added `engine_mcp_tool(name, args_json) -> napi::Result<String>`. Dispatches to per-tool handlers using `runtime.block_on`. |
| `frontends/holon-worker/web/worker-entry.mjs` | Added `engineMcpTool` case to the dispatch switch. |

---

## End-to-End Test Result

Tested with a Node.js mock browser (`role=browser`) responding to all tool calls:

```
[test] initialize: { name: 'rmcp', version: '0.12.0' }
[test] session: 7d3d2f96-4b5d-45f1-bd33-bbb55f402d0e
[test] notifications/initialized HTTP status: 202
[test] tools/list: 33 tools (list_commands, click, inspect_loro_blocks...)
[test] execute_query result: {"content":[{"type":"text","text":"mock result for
       tool=execute_query args={\"sql\":\"SELECT id FROM block LIMIT 5\",
       \"language\":\"sql\"}"}],"isError":false}
[test] PASS ✓
```

The full relay path was exercised: HTTP → holon-mcp → WS hub → mock browser → WS hub → holon-mcp → SSE response.

**Not yet tested**: real Dioxus browser page with live WASM. The `trunk build` completed
successfully. The next step is opening `http://127.0.0.1:8766`, waiting for boot, and
observing `[mcp-hub] browser connected` in serve.mjs logs, then running a real `execute_query`.

---

## How to Run

```bash
# Terminal 1 — dev server with watch
cd frontends/dioxus-web
PORT=8766 node serve.mjs --watch

# Terminal 2 — native relay (browser relay mode)
HOLON_BROWSER_RELAY_URL=ws://127.0.0.1:8766/mcp-hub \
RELAY_PORT=3002 \
cargo run -p holon-mcp

# Terminal 3 — open browser, then test
open http://127.0.0.1:8766
# wait for [mcp-hub] browser connected in Terminal 1, then:
node test-relay.mjs   # (recreate from devlog or use curl)
```

Claude Code MCP config:
```json
{ "mcpServers": { "holon": { "url": "http://localhost:8766/mcp" } } }
```

---

## Key Design Decisions & Gotchas

**Single port**: serve.mjs proxies `/mcp` to `RELAY_PORT`. Claude Code only needs one URL.

**No schema duplication**: `BrowserRelayServer` calls
`(HolonMcpServer::tool_router_ui() + HolonMcpServer::tool_router_backend()).list_all()`.
These are associated functions — no instance needed.

**Reconnect on both sides**: Both the native relay (tokio-tungstenite connection loop) and
the browser page (`onclose` → `gloo_timers::TimeoutFuture(1000)`) reconnect automatically.
This handles `trunk --watch` restarts without requiring a page reload or relay restart.

**Binary vs text WebSocket frames**: The `ws` npm library's `message` event passes
`(data, isBinary)`. The hub must forward with `peer.send(data, { binary: isBinary })` so
text frames stay as `Message::Text`. The Rust relay's read loop only handles `Message::Text`
(other variants are `continue`d). This was the final bug fixed during testing.

**`notifications/initialized` needs `Accept` header**: rmcp's StreamableHTTP returns
`406 Not Acceptable` if the `Accept: application/json, text/event-stream` header is missing
on any POST, including notifications.

**`engine_mcp_tool` uses `block_on`**: The worker is single-threaded WASM. All async ops
inside `engine_mcp_tool` must go through `runtime.block_on`. For subscription-based tools
(`watch_query`, `poll_changes`), `block_on(sleep(Duration::ZERO))` must be called before
reading the pending buffer to let the reactor drain.

**Loro/org tools**: `inspect_loro_blocks`, `diff_loro_sql`, `list_loro_documents`,
`read_org_file`, `render_org_from_blocks` return a "not supported" error. These require
`LoroDocumentStore` wired into the worker — deferred.

---

## What to Review

1. **`frontends/mcp/src/browser_relay.rs`** — Core of the feature. Check:
   - Reconnect loop: does it correctly drain `pending` on disconnect?
   - `forward()`: lock ordering (ws_tx → pending), no deadlock?
   - `dispatch_response()`: is the `content_str` / `Vec<Content>` parsing correct?
   - `BrowserRelayServer::list_tools`: does `ListToolsResult::with_all_items` set all fields correctly?

2. **`frontends/dioxus-web/serve.mjs`** — Hub routing and proxy:
   - Is the `{ binary: isBinary }` forwarding correct and sufficient?
   - Does the SSE proxy handle early client disconnect without crashing?
   - Are the COOP/COEP headers preserved through the proxy?

3. **`frontends/dioxus-web/src/main.rs`** — `connect_mcp_relay`:
   - Are the `Closure::forget()` calls correct (no leak)?
   - Does the `MCP_WS` thread-local get properly cleared on disconnect?
   - Is `wasm_bindgen_futures::spawn_local` the right executor for the message handler?

4. **`frontends/holon-worker/src/lib.rs`** — `engine_mcp_tool`:
   - Are all the tool dispatch arms correct (especially `list_tables`, `describe_ui`)?
   - Is the `block_on` usage safe for the WASM single-thread model?
   - Does `poll_changes` correctly call `block_on(sleep(ZERO))` before draining?

5. **`frontends/mcp/src/di.rs`** — Relay mode branch:
   - Does the early-return path in relay mode leak any resources?
   - Is the `BrowserRelayServer` factory closure (`move || Ok(...)`) correct for
     StreamableHTTP's session-per-request model?
