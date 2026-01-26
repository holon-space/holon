# Handoff: MCP `now-query` returns 0 rows on first call (matview empty on first read)

Status: **partial fix landed, root cause still unresolved**.

## TL;DR

`mcp__holon-mcp__execute_source_block` (and `execute_query`, `execute_raw_sql`)
against the `block` matview returns **0 rows on the first call after MCP
server start**, then **3 rows on every subsequent call**. The data is in the
matview either way — the first read just doesn't see it.

I hypothesised "OrgSync ingestion lag" and added a one-shot
`OrgSyncIdleSignal::wait_quiescent` gate via `HolonMcpServer::ensure_warmed`.
Logs prove the wait actually fires (25s on a fresh start, until quiescence).
**It does not fix the bug** — the first matview SELECT after the wait
still returns 0, while a `block_raw` / `block` count check immediately
afterwards shows 968 rows in both.

So the bug is *not* about ingest backlog. Something on the read path of the
first materialised-view cursor returns an empty result set on a populated
matview.

## How to reproduce

1. Make sure `.mcp.json` points at the new reloaderoo + cargo-watch flow
   (see "Dev workflow" below) so a fresh `holon-mcp` is spawned.
2. `/mcp reconnect` (or kill `holon-mcp` so reloaderoo respawns it).
3. First MCP call:
   ```
   mcp__holon-mcp__execute_source_block(block_id="block:now-query::src::0")
   → row_count: 0
   ```
4. Second MCP call (same args, immediately after):
   ```
   → row_count: 3
   ```

The same SQL via `execute_raw_sql` against `block` (matview) the first time
also returns 0 (verified earlier in the session before the fix attempts).
Once one matview-touching query has run, all subsequent ones return data.

## What's been ruled out

| Hypothesis | How tested | Result |
|---|---|---|
| Org ingestion not finished when first query runs | `wait_for_ready=true` in `SessionConfig` (was `.without_wait()`) | Still 0 |
| OrgSync loop still firing events when first query runs | `OrgSyncIdleSignal::wait_quiescent(500ms, 30s)` in `ensure_warmed` (logged 25s wait → quiescent=true, then query) | Still 0 |
| Bug is in `execute_source_block`'s extra `block_raw` lookup | First call to `execute_query` and `execute_raw_sql` (against `block`) shows the same pattern | Same 0-then-3 pattern |
| `compile_query` mangles the SQL | `compile_query` returns byte-identical SQL to the source-block content | Not the bug |
| Stored `source_language` parses wrong | Block has `source_language='holon_sql'`; explicit `language` override produced 3 rows the second time, but first call still 0 | Not it |

What this leaves: **the matview cursor returns 0 rows on first open**, even
when the matview has been incrementally populated. After one cursor open,
something gets warmed and subsequent opens are fine.

## Strong remaining suspects

1. **Turso `MaterializedViewCursor` first-open laziness.** There's a
   matching memory entry: "Turso `MaterializedViewCursor::ensure_tx_changes_computed`
   didn't walk upstream matview deltas when reading inside an open txn —
   fixed in `7cf0a2e68a3a`." We're outside an explicit txn (autocommit), but
   maybe the *first* cursor-open after matview creation needs a similar
   compute step that's currently no-op'd in autocommit too. Worth filing
   another upstream repro.
2. **DDL race on schema-modules → first SELECT.** `BlockMatviewSchemaModule`
   creates the `block` matview (`reconcile_named_view`, schema_modules.rs:181)
   before ingestion writes happen. When the matview is created on an empty
   `block_raw`, its initial state is empty. CDC then propagates inserts.
   Possibility: the cursor-side cache for that view is initialised to "empty"
   at create time and isn't invalidated by the CDC inserts on first read.
3. **Connection-level prepared-statement cache.** `BackendEngine::execute_query`
   already retries on `"Database schema changed"` errors with a 50ms backoff,
   suggesting prior pain in this area. Maybe the first cursor opens against
   a stale prepared statement and silently returns 0 instead of erroring.

## What to try next

- Add tracing inside Turso's `MaterializedViewCursor::next` / `ensure_tx_changes_computed`
  to see what state the cursor is in on the first open vs the second.
- Build a minimal repro (no MCP, no orgmode) that:
  1. CREATE TABLE block_raw, block_tags, block_requires
  2. CREATE MATERIALIZED VIEW block AS (the dual-LEFT JOIN + json_group_array)
  3. INSERT ~1000 rows into block_raw + a few into the junctions
  4. Open a fresh connection
  5. SELECT * FROM block WHERE … with a filter that should match
  6. Assert: first SELECT returns N rows (not 0)

  If the minimal repro shows the same 0-then-N pattern, file upstream and
  drop a `bigdata/turso/bugs/holon_block_matview_first_open_empty_2026-05-08.{sql,md}`.
- As a holon-side workaround until the upstream fix lands: add a `SELECT 1
  FROM block LIMIT 0` warmup query at startup (after schema reconcile +
  initial ingest) so the *first* application-visible query is never the
  first cursor open. Cheaper and more targeted than `wait_quiescent`.

## Current state of the code (changes I left in)

These are NOT a fix — they were the warmup-race hypothesis, but they're
useful infrastructure for the eventual fix and the second-call works correctly.

### `.mcp.json` — switched to reloaderoo + direct binary

```json
{
  "mcpServers": {
    "holon-mcp": {
      "type": "stdio",
      "command": "npx",
      "args": ["reloaderoo", "proxy", "--",
        "./target/debug/holon-mcp",
        "--stdio",
        "--orgmode-root", "/Users/martin/Workspaces/pkm/holon-pkm",
        ":memory:"
      ]
    }
  }
}
```

`reloaderoo` keeps the stdio link to Claude Code stable; only the child
process gets respawned on rebuild.

### `scripts/dev-mcp.sh` — cargo-watch loop

Watches `crates/` and `frontends/mcp/`. On change: `cargo build --bin
holon-mcp` then `pkill -x holon-mcp`. reloaderoo's auto-restart respawns
the child with the freshly-built binary. The user runs this in a separate
terminal.

### `frontends/mcp/src/main.rs`

- Removed `.without_wait()` on `SessionConfig` so the bootstrap waits for
  `FileWatcherReadySignal` (initial-scan ingest done). Comment in the source
  documents the reasoning.
- Added `injector.try_resolve::<holon_orgmode::OrgSyncIdleSignal>()` and
  passes it to `run_stdio_server`.
- `run_stdio_server` now takes `idle_signal: Option<Arc<OrgSyncIdleSignal>>`
  and constructs `HolonMcpServer::with_type_registry_and_idle(...)`.
- Pre-existing `parse().ok()` got an `// ALLOW(ok): non-critical env var
  parse` so the archlint hook stops blocking unrelated edits.

### `frontends/mcp/src/server.rs`

`HolonMcpServer` gained two fields:

```rust
pub(crate) idle_signal: Option<Arc<holon_orgmode::OrgSyncIdleSignal>>,
pub(crate) warmup: Arc<tokio::sync::OnceCell<()>>,
```

…a new `with_type_registry_and_idle` constructor, and:

```rust
pub(crate) async fn ensure_warmed(&self) {
    let Some(signal) = self.idle_signal.clone() else { return };
    let warmup = self.warmup.clone();
    warmup.get_or_init(|| async move {
        signal.wait_quiescent(
            std::time::Duration::from_millis(500),
            std::time::Duration::from_secs(30),
        ).await;
    }).await;
}
```

`OnceCell` ensures the wait fires at most once per process; subsequent
tool calls hit a no-op fast path.

### `frontends/mcp/src/tools.rs`

`self.ensure_warmed().await;` added at the top of `execute_query`,
`execute_source_block`, `execute_raw_sql`.

### Things to consider reverting

If the "matview-first-open" theory pans out and you end up adding a startup
warmup SELECT instead, the `ensure_warmed` plumbing becomes unnecessary
(and adds 25-30s to the first user-visible call when nothing else has
happened to advance the OrgSyncIdleSignal tick — uncomfortably long).
Keep it only if there's a separate use case for "wait for sync to settle";
otherwise revert.

## Useful debugging artefacts

- Latest holon-mcp log with the proven-to-fire ensure_warmed wait:
  `/var/folders/hc/2q6czxpx6j9_87bq787752jw0000gn/T/holon-mcp-1778197454.log`
  (search for `awaiting OrgSync quiescence on first query` and the
  matching `OrgSync quiescent=true` ~25s later).
- The Now-query block content (`block:now-query::src::0` content):

  ```sql
  SELECT b.*
  FROM block b
  WHERE json_extract(b.properties, '$.task_state') = 'TODO'
    AND json_extract(b.properties, '$.gate') = 'G1'
    AND NOT EXISTS (
      SELECT 1 FROM block_requires br
      JOIN block bl ON bl.id = br.required_id
      WHERE br.block_id = b.id
        AND COALESCE(json_extract(bl.properties, '$.task_state'), '') <> 'DONE'
    )
    AND (
      EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'agent')
      OR NOT EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'human-only')
    )
  ORDER BY
    json_extract(b.properties, '$.priority'),
    json_extract(b.properties, '$.effort'),
    b.id
  LIMIT 10
  ```

- Expected result (3 rows): `block:open-questions-inbox`,
  `block:edge-field-descriptor`, `block:claude-sessions-under-topics`.
  `block:handoff-md-migration` is correctly excluded — has 1 open
  requirement.
