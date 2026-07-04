# Mock-MCP E2E: challenging-behaviour catalogue

The generic MCP connector (`holon-mcp-client`) is wired end-to-end
(`docs/integrations/*.yaml` sidecar → `IntegrationFileConfig` →
`build_mcp_integration` → `McpSyncEngine` → `QueryableCache` → Turso IVM) but
had **no E2E test against a live MCP server** — only view-level tests
(`crates/holon-turso/tests/sidecar_views.rs`) and in-process `duplex` fakes
(`holon-integration-tests::pbt_mcp_fake`), which never spawn a real process nor
speak MCP over stdio.

Per the ruling, we do **not** test any specific real server (Claude Code,
Todoist, …). Our client must work for **any** MCP server, so instead we ship a
configurable **mock MCP server binary** (`crates/holon-mcp-mock`) that speaks
real MCP over stdio (the `transport-child-process` path our client uses) and can
be toggled — via the `MOCK_MCP_SCENARIO` env var set in the sidecar's
`child_process.env` — into each of the challenging behaviours observed in real
servers. The E2E tests (`crates/holon-mcp-mock/tests/mcp_mock_e2e.rs`) drive the
**production** entry point `build_mcp_integration` with a YAML sidecar pointing
at that binary, then query the resulting cache table / IVM views.

Fail-loud contract (`CLAUDE.md`): a protocol/data violation from the server must
surface as an **error** out of `build_mcp_integration` or `sync_all`, never as a
silently-empty cache.

## Catalogue

| # | Behaviour | Real-world source | How the mock exhibits it | Consuming test |
|---|-----------|-------------------|--------------------------|----------------|
| 1 | Well-formed tool sync | Todoist `find-tasks`, most tool servers | `happy`: `list-items` tool returns `{items:[…]}` as a JSON text block | `happy_tool_sync_populates_cache` |
| 2 | Slow / high-latency response | Hosted servers behind cold-start / network (SLO-relevant) | `slow`: `call_tool` sleeps 300 ms before answering | `slow_response_is_tolerated` |
| 3 | Large result set | `find-tasks filter:all`, GitHub list APIs | `large`: one `call_tool` returns 1500 records, no cursor | `large_result_set_syncs_fully` |
| 4 | Cursor / pagination | Todoist `nextCursor`, GitHub `Link` cursors | `paginated`: page size 2 over 5 items; each page echoes an offset `nextCursor`; client advances one page per `sync_all` (incremental append) | `cursor_pagination_accumulates` |
| 5 | Malformed (non-JSON) payload | Servers that wrap errors in prose, truncated bodies | `malformed_json`: tool text block is not valid JSON | `malformed_json_fails_loud` |
| 6 | Schema-violating payload (missing extract field) | Server renames/omits the documented array field | `missing_field`: returns `{other:[…]}`, no `items` | `missing_extract_field_fails_loud` |
| 7 | Dual content block (JSON + trailing prose) | Official Todoist MCP at `ai.todoist.net/mcp` returns a JSON block **and** a human summary block | `dual_text_block`: two text blocks — JSON then prose | `dual_text_block_still_parses` |
| 8 | Tool-level error result (`isError:true`) | Any tool that reports a domain error via `CallToolResult.is_error` | `tool_error`: returns `CallToolResult::error` | `tool_error_result_fails_loud` |
| 9 | Stateful resource changing between polls | Poll-based resources whose content mutates server-side | `stateful`: `read_resource` returns 1 item on the first read, 2 on the next | `stateful_resource_between_polls` |
| 10 | `resources/subscribe` + push notification | Servers advertising `resources.subscribe` that emit `notifications/resources/updated` | `subscribe_push`: advertises subscribe capability; on `subscribe`, spawns a task that mutates state and pushes an `updated` notification | `subscribe_push_updates_cache` |
| 11 | Broken initialization handshake / protocol violation | Version-mismatched or crashing servers that fail `initialize` | separate raw-stdio binary `mock-mcp-raw` replies to `initialize` with a JSON-RPC error | `handshake_error_fails_loud` |

## Deliberately NOT mocked (gaps)

- **Mid-session disconnect / process crash after connect.** The child exiting
  mid-sync is a distinct fault-injection axis (tests `McpRunningService` drop +
  reconnect policy, which the client does not yet implement). Left for a
  follow-up once a reconnect policy exists — mocking it now would only assert
  today's "connection dies → background task ends" non-behaviour.
- **Protocol-version *negotiation* (as opposed to hard handshake failure).**
  rmcp negotiates `protocolVersion` internally; asserting a specific
  down-negotiation is an rmcp-internal concern, not our connector's. #11 covers
  the fail-loud edge (server refuses `initialize`); graceful version stepping is
  rmcp's contract to keep.
- **`notifications/tools/list_changed` / dynamic tool re-discovery.** Our sync
  path keys off resource updates, not tool-list changes; there is no consumer to
  assert against yet.
- **OAuth / auth-challenge handshakes.** Exercised separately via the
  `AuthMode::OAuth` path and `PendingOAuthFlows`; orthogonal to the transport
  behaviours catalogued here. The mock speaks `AuthMode::None`.
- **Deferred/lazy tool-schema fetching.** The connector reads schema from the
  sidecar YAML (and auto-discovers from resource templates), not from
  incremental `tools/list` schema streaming, so there is no code path to drive.
