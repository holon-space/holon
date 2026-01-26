# Phase 4: LiveData Arc dedup + navigation retention caps (matview GC deferred)

Date: 2026-06-11. Continues the memory-reduction plan
(`~/.claude/plans/shiny-watching-newt.md`).

## LiveData<T> stores Arc<T>

`crates/holon/src/sync/live_data.rs`: `items` is
`MutableBTreeMap<String, Arc<T>>`; CDC apply and snapshot reads now clone
Arcs instead of deep-cloning values (DBSP/Block clone churn through the 5
mirrors was a measured hotspot — see
`devlog/2026-06-11-memory-measurement.md`). Owned clones survive only at
true ownership boundaries (`BlockSnapshot::from_ordered`, the
`DocumentManager` trait surface). Consumers updated:
turso_block_query_source (sorts `Arc<OrderedBlock>`), holon-orgmode
`LiveDocumentManager`, integration-test capabilities, holon tests.

## navigation_history retention cap

`NavigationProvider::focus` step 6 keeps only the 100 most recent CLOSED
rows per region: `sql/navigation/get_prune_threshold.sql` (threshold id in
Rust because Turso lacks subquery-DELETE) + `prune_closed_history.sql`.
Open rows and pins untouched; `focus_roots` consumes only open rows so
the matview is unaffected. Back/forward beyond the window is forgotten by
design. The in-memory provider mirrors the cap (101 entries incl. current;
unit test `focus_history_is_capped`).

## Matview GC: DEFERRED deliberately

`HOLON_MATVIEW_GC` (drop matviews for queries no longer watched) was last
in the plan because it needs a `/turso-sql-replay` soak — Turso IVM has a
history of fragility around matview lifecycle (chained-matview hangs,
context-param preload). Now that live_query shells actually `unwatch`
their query watchers (see `devlog/2026-06-11-livequery-streaming.md`),
a GC has a real signal to key off: registry removal in
`ReactiveEngine::unwatch` is the hook point. Do it in a dedicated session
with watch_query open/close soak via the holon MCP + turso-sql-replay.

## Gates

e2e `general_e2e_pbt` 2/2 PASS (sql_only + Full, ~61s each); gherkin
replay 8 steps; frontend/gpui/layout units green; holon lib tests incl.
new cap test green; live_data 13/13, holon-orgmode 17/17.
`holon::stress_tests`/`integration_tests` iroh-sync family timeouts
(three_peer_synchronization, parallel_sync_operations) are PRE-EXISTING —
identical failures in the 2026-06-10 session log
(`~/.claude/jobs/f9fe225e/tmp/unit_mine.log`: 26 failed + 4 timed out)
before any of today's changes.
