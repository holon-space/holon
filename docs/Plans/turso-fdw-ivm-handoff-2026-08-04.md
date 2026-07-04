# Turso handoff: FDW as an IVM source (chat-view zero-bubble blocker)

Date: 2026-08-04 · Repos: holon `/Users/martin/Workspaces/pkm/holon`, Turso fork
`/Users/martin/Workspaces/bigdata/turso` (pin `9d7394fb`) · Status: **investigated, verdict below**

## TL;DR — the premise is refuted, and the fallback is the architecture

1. **Turso does NOT refuse `CREATE MATERIALIZED VIEW` over a foreign table.** The exact
   statement Holon emits for the chat view — `IF NOT EXISTS`, projection with alias,
   `WHERE session_id = '…' AND role IS NOT NULL` — creates successfully and returns the
   correct rows. Proven by a new 6-rung reproducer (below), 5 of 6 rungs green.
2. **A matview over a foreign table is a SNAPSHOT, not a stream.** The one red rung is
   exactly the property the chat view needs: a new row appearing at the foreign source is
   invisible to the matview until `REFRESH MATERIALIZED VIEW`. This is structural, not a bug.
3. Therefore **FDW-as-IVM-source is the wrong target**. Recommendation: point the profile at
   the `cc_message` **cache table**, which is not a fallback — it is the path
   `MatviewManager` was already built for (`prime_fdw_caches`, `register_fdw_table`,
   `vtable.write_through: true`). Poll/refetch is not needed and should not be considered.
4. The BugFunnel entry (2026-08-03 I6) states Turso "refuses" the DDL. That sentence is
   wrong and should be corrected; the recorded error was Holon's outer `.with_context`
   wrapper and the **inner Turso error was never captured**. See "Open thread" below.

## Reproducer

New file, uncommitted, in the Turso fork:

- `/Users/martin/Workspaces/bigdata/turso/tests/integration/query_processing/test_fdw_matview_holon_shape.rs`
- one-line registration added to
  `/Users/martin/Workspaces/bigdata/turso/tests/integration/query_processing/mod.rs`
  (`mod test_fdw_matview_holon_shape;`)

Run:

```
cd /Users/martin/Workspaces/bigdata/turso
cargo test -p core_tester --test integration_tests fdw_matview 2>&1 | tee /tmp/fdw-repro.log
```

It builds a CSV-backed `cc_message_fdw (uuid, session_id, role, content, timestamp)` and adds
one clause at a time so a failure names the clause rather than the feature.

Result (2026-08-04, pin-era tree): **12 passed, 1 failed.**

| Rung | Shape | Result |
|---|---|---|
| 1 | `CREATE MATERIALIZED VIEW IF NOT EXISTS … SELECT * FROM fdw` | ok |
| 2 | projection + `timestamp AS ts` alias | ok |
| 3 | `WHERE session_id = 's1'` | ok |
| 4 | `WHERE role IS NOT NULL` | ok |
| 5 | **full Holon statement** (ORDER BY already removed by `strip_order_by`) | **ok** |
| 6 | append a row to the foreign source, re-query without `REFRESH` | **FAILED** — 3 rows, expected 4 |

The pre-existing `test_fdw_matview.rs` (already at the pinned rev, blob `e79dca1f`) independently
confirms this: `test_refresh_matview_on_fdw` *asserts* the matview keeps showing stale data until
`REFRESH MATERIALIZED VIEW` is issued.

## Localization in the Turso fork

FDW-as-matview-source is implemented, not rejected:

- `core/incremental/view.rs:394` — `Table::Virtual(vtab) => Some(Self::from_virtual(vtab))`;
  foreign tables are accepted as `ReferencedTable`s.
- `core/incremental/view.rs:410-418` — `from_virtual` sets `is_virtual: true`,
  `has_rowid: true`, no rowid alias (synthetic rowids).
- `core/incremental/view.rs:1175` — population emits `SELECT *` (no `, rowid`) for virtual sources.
- `core/incremental/view.rs:1988` — `extract_rowid_and_values` assigns a synthetic rowid per
  virtual-table row.

Why it cannot *stream* — the delta feed does not exist for a foreign table:

- `core/vdbe/execute.rs:10874-10891` — deltas are recorded by the **btree** DML opcodes
  (`Insn::Insert` and friends) into `connection.view_transaction_states`, per dependent view.
- `core/vdbe/mod.rs:2255-2285` — at commit, those per-table deltas are assembled into a
  `DeltaSet` and pushed through `IncrementalView::merge_delta` → the DBSP circuit.
- `core/vdbe/execute.rs:1479-1565` — `Insn::VUpdate`, the *only* write path for a virtual
  table, never touches `view_transaction_states`. No delta is ever produced.

So the IVM circuit's input is "rows written through this connection's local btree DML". A
foreign table has no such write path at all — in Holon's case rows materialise inside
`xFilter` from an MCP call, entirely outside any transaction. There is nothing to hook.
`REFRESH MATERIALIZED VIEW` (`core/translate/view.rs:497+`) exists precisely to fill that gap
by re-running population from scratch.

Prior art consulted: skills `turso-chained-matview-hang` (matview-on-matview) and
`turso-ivm-context-param-preload` (`$param` in DDL). Neither applies here.

## Feasibility verdict: DO NOT build FDW-as-IVM-source

Making it real requires the foreign source to emit deltas, which means all of:

1. A change-feed contract on the `ForeignDataWrapper` trait (`xChanges` / subscription), which
   every driver must implement. CSV cannot. MCP could only do so via server-side notifications.
2. Stable identity across refetches. Foreign rows currently get **synthetic rowids assigned by
   scan order** (`view.rs:1988`). A DBSP retraction must carry the *same* key as the earlier
   insertion, so ordinal rowids make correct retraction impossible — a real primary-key
   contract for foreign tables is a prerequisite.
3. A place to run maintenance. Deltas are applied at commit inside a VDBE transaction; an
   out-of-band foreign-source push has no transaction, no pager access, and no ordering
   guarantee against concurrent local writes.

That is a multi-week change to the fork's core contracts, with a durability/correctness blast
radius across every matview. **Disproportionate.** Especially since Holon already has a
delta-producing mirror of the same data.

### Recommended option (Martin's "cc_message" route) — already the designed path

`message` in `docs/integrations/claude-history.yaml:389-419` declares `vtable.write_through:
true`. That means the FDW writes fetched rows into the **`cc_message` btree cache table**, and
`MatviewManager` already knows how to drive it:

- `crates/holon-turso/src/matview_manager.rs:405` `register_fdw_table(cache_table)`
- `crates/holon-turso/src/matview_manager.rs:585` / `:884` `prime_fdw_caches(sql)` — for each
  table in the watch SQL that has an FDW counterpart, it runs the *same* query against
  `{table}_fdw` first, which forces the fetch and the write-through, then creates the matview.

`prime_fdw_caches` keys on the **cache** table name (`cc_message`). The shipped profile names
`cc_message_fdw` directly, so it matches nothing, the cache is never primed (consistent with
the observed `SELECT count(*) FROM cc_message` = 0), and the matview — even when it is created
successfully — is a one-shot snapshot of whatever the FDW returned at that instant.

Change required: in `docs/integrations/claude-history.yaml`, lines **76**, **207** and **337**,
replace `cc_message_fdw` → `cc_message` and `cc_agent_message_fdw` → `cc_agent_message`. Then
the matview sits on a real btree table, `prime_fdw_caches` fetches through the FDW on demand,
write-through inserts fire the btree DML path, and the DBSP circuit streams. No Turso change.

The `$context_id` scheme-prefix mismatch documented in the same BugFunnel row
(`cc-session:…` vs raw ids, sidecar strips with `substr(cc_session.id, 12)`) is independent and
must be fixed alongside.

## Open thread (Holon-side, not Turso)

The inner Turso error behind `Failed to create materialized view watch_view_9ef7f36587fc903c`
was never recorded — `crates/holon-turso/src/matview_manager.rs:670-675` wraps it in
`.with_context(...)` and only the outer line reached the ledger. Since the identical statement
is proven to succeed in Turso, the real cause is Holon-side and still unknown. Most likely
candidates, in order: the FDW's own `xFilter`/MCP call failing during matview **population**
(surfacing as a DDL error), or the 120s dependency timeout in
`crates/holon-turso/src/turso.rs:604`. Before any further Turso work, log the full error chain
(`{:#}`) at that site and re-run the chat view.

## Acceptance criteria

The chat view is fixed when, with `crates/holon-frontend/tests/chat_view_render.rs` **no longer
stubbing `watch_query`**:

1. Opening a `session` / `live_session` entity profile against a real Turso + real
   claude-history MCP creates the watch matview without error.
2. The matview reports **rows > 0** for a session with known messages — the oracle is
   `rendered bubbles == query rows`, not merely "a node exists".
3. A message arriving at the MCP source after the view is open appears in the UI **without**
   any `REFRESH MATERIALIZED VIEW` and without a poll — i.e. it streams through CDC.
4. `SELECT count(*) FROM cc_message` is non-zero after the view opens (write-through fired).
5. No `_fdw` table name appears in any `live_query` SQL in `docs/integrations/*.yaml`.

If a Turso change is ever revisited, its acceptance criterion is rung 6 of
`test_fdw_matview_holon_shape.rs` going green unchanged.

## Files created (all uncommitted, Turso fork)

- `tests/integration/query_processing/test_fdw_matview_holon_shape.rs` (new)
- `tests/integration/query_processing/mod.rs` (one `mod` line added)

## Verification addendum (2026-08-04, independent two-lane audit)

Both sides re-verified at Turso `e36d700507c4` (git `98baf220`) and current holon tree.
**Verdict: the doc holds; the repoint recommendation is feasible — but "poll/refetch is
not needed and should not be considered" is contingent, not settled.**

Turso lane (adversarial, refutation attempted and failed):

- Reproducer re-run: `test result: FAILED. 12 passed; 1 failed` — sole failure is rung 6
  (`test_fdw_matview_holon_shape.rs:147`, "3 ≠ 4 without REFRESH"). Note: 12+1 is the
  combined `fdw_matview` filter (6 new + 7 pre-existing tests); of the new file, 5/6 green.
- CSV-caching confound ruled out by live CLI probe: the bare foreign table sees a mid-session
  append on the SAME connection; only the matview stays stale. Rung 6 genuinely tests streaming.
- `Insn::VUpdate` delta-free confirmed globally (all `view_transaction_states` writers are in
  `op_insert`/`op_delete`), and **stronger than the doc claims**: FDW tables are engine-read-only
  (`VirtualTable::readonly()` = true for `Internal` vtabs, `core/vtab.rs:38-44`; the
  `ForeignDataWrapper` trait has no write method), so `INSERT INTO <fdw>` errors `ReadOnly` —
  no engine write path exists at all for a foreign table.

Holon lane (all anchors confirmed; three findings the doc lacks):

1. **Write-through crux CONFIRMED**: `McpCursor::filter` → `WritebackTarget::write_rows`
   (`crates/holon-mcp-client/src/mcp_vtable.rs:694-732`) executes literal
   `INSERT OR REPLACE INTO cc_message …` on a real core connection → btree DML → DBSP streams.
   Residual: confirm `WritebackTarget.conn` is the same DB/handle as the matview's (unlikely
   to differ, untraced to certainty).
2. **"No poll needed" has two unverified preconditions.** A real non-poll path exists
   (`on_fdw_primed` → MCP `subscribe` → `resources/updated` → `resync_by_uri` → write-through,
   `mcp_sync_engine.rs:655-705`, `:501-529`), BUT (a) it requires the external
   claude-code-history-mcp server to actually push `resources/updated` for message resources —
   unverifiable from the holon repo; and (b) `crates/holon-app/src/wiring.rs:338-341` installs
   only `integrations().first()` as the matview hook — if claude-history isn't first, the
   subscribe silently no-ops. No fallback poll interval is configured for `message`/
   `live_session` in the yaml, so if either precondition fails the system is prime-once-and-
   never-refresh and acceptance criterion 3 fails. Both are cheap pre-flight checks; do them
   before treating "no poll" as settled.
3. Additional risks: failed prime is warn-and-continue (`matview_manager.rs:923-928`) → a
   transient MCP failure at first open yields a permanently empty view until a notification
   fires; `prime_fdw_caches` rewrites SQL via plain substring replace (`:905`) — latent footgun
   for complex SQL; the "swallowed error" claim overstates slightly — `.with_context` chains
   the inner error, visibility depends on the caller's log format (`{}` vs `{:#}`).

Amended pre-flight for the fix: (i) verify the MCP server emits `resources/updated` for
`claude-history://sessions/{id}/messages`; (ii) verify claude-history's `McpSyncEngine` is the
installed matview hook (or fix the `first()`-only wiring); (iii) make prime failures loud.
