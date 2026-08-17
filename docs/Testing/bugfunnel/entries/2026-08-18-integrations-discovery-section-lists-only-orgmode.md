---
id: 2026-08-18-integrations-discovery-section-lists-only-orgmode
date: 2026-08-18
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  The left-sidebar Integrations discovery section lists only `orgmode` even
  though gcal, gmail, todoist and claude-history are enabled and syncing,
  because it reads `sync_states` — a sync-CURSOR table that only cursor-based
  providers ever write.
---

## Bug

Found by Martin dogfooding the live GPUI app (cold boot 2026-08-17 22:07,
`/tmp/holon-cold.log`), right after enabling gcal / gmail / todoist /
claude-history through the D4.b enablement cutover state files.

The left-sidebar **Integrations** discovery section rendered exactly one row:
`orgmode`. The same boot log proves claude-history was actively live-syncing
and todoist was connected and issuing operations, so the section was not
merely stale — it structurally cannot see those providers.

## Root cause

The seeded section is a `live_query` over `sync_states`
(`assets/default/index.org:12`):

```
live_query(#{sql: "SELECT provider_name, updated_at FROM sync_states ORDER BY provider_name ASC", ...})
```

`sync_states` is **not** a registry of enabled or connected integrations. It is
the sync-**cursor** persistence table, written only via
`SyncTokenStore::save_token` (`crates/holon-core/src/traits.rs:2766`). Two
independent facts make it list at most `orgmode`:

1. **`save_token` is on the incremental branch only.** In
   `McpSyncEngine::sync_entity_inner`
   (`crates/holon-mcp-client/src/mcp_sync_engine.rs:399-412`), the token is
   saved inside `if let Some(new_cursor) = fetch_result.new_cursor`. A provider
   whose strategy returns no cursor takes the `apply_full_sync` arm and never
   writes a row. Concretely:
   - `ResourceSync::fetch_records`
     (`crates/holon-mcp-client/src/mcp_sync_strategy.rs:285-290`) ignores the
     token store entirely (`_: &dyn SyncTokenStore, _: &str`) and never
     produces a cursor. **claude-history** is `list_resource`-only across all
     its entities (`assets/integrations/claude-history.yaml`) — so it can never
     appear.
   - `ToolSync::fetch_records`
     (`crates/holon-mcp-client/src/mcp_sync_strategy.rs:184`) produces a cursor
     only when the sidecar declares `cursor:`. **gcal** and **gmail** declare
     `list_tool` with no `cursor:` block (`assets/integrations/gcal.yaml:179,227`,
     `assets/integrations/gmail.yaml:208,237,259`) — so they can never appear
     either.

2. **The key is a token key, not a provider name.** `sync_entity_inner` builds
   `token_key = format!("{}.{}", provider_name, entity_name)`
   (`mcp_sync_engine.rs:372`) and saves under *that*. So even **todoist**,
   which does declare `cursor:` (`assets/integrations/todoist.yaml:50,70`),
   would surface as two rows named `todoist.todoist_tasks` and
   `todoist.todoist_projects` — never as one integration called `todoist`.

`orgmode` is the sole provider that writes a bare provider name, because
`OrgModeSyncProvider` calls `save_token(self.provider_name(), ...)` directly
(`crates/holon-orgmode/src/orgmode_sync_provider.rs:355,445`).

**Log evidence** (`/tmp/holon-cold.log`, 4048 lines): 37 occurrences of
`sync_entity: fetched records`, and **zero** occurrences of `Saved cursor`. The
only `sync_states` write in the entire boot is
`[DatabaseSyncTokenStore] Saved sync token for provider 'orgmode'`. The section
is rendering its query faithfully; the query is over the wrong table.

## Regression or pre-existing

**Pre-existing gap, newly exposed** — not a regression from the D4.b cutover
(`93800cc2`).

A/B reasoning: `jj diff -r 93800cc2 --summary` touches no file on the
`sync_states` write path. `mcp_sync_engine.rs`, `mcp_sync_strategy.rs`,
`sync_token_store.rs`, `orgmode_sync_provider.rs` and `assets/default/index.org`
are all untouched. The only two sidecars it edits (`gcal.yaml`, `gmail.yaml`)
receive **comment-only** changes to the activation instructions — no `cursor:`
config was added or removed. The cutover changed how an integration becomes
*enabled*; it never touched how one becomes *visible*.

What the cutover did was make the defect observable for the first time: before
it, Martin had no integrations switched on, so a discovery section showing only
`orgmode` was indistinguishable from correct.

## Missing piece

There is **no queryable projection of integration enablement or connection
state**. `IntegrationConfigStore` holds `Mutable<IntegrationState>` cells backed
by filesystem state files (`crates/holon-mcp-client/src/integration_state.rs:160-229`)
and is never mirrored into a Turso table, so no `live_query` — the only
mechanism the seeded layout has — can read it. `sync_states` was picked as the
nearest available table, and it is the wrong one.

The escape is primarily **ENVIRONMENT**: the failing path is
`McpSyncEngine::sync_entity_inner` running a cursorless strategy against a real
sidecar, and that path does not exist in the keystone's wiring at all (see
below). Secondarily **ORACLE**: no invariant anywhere relates the contents of
the Integrations section to the set of enabled integrations, so even a keystone
that did run MCP sync would not have gone red.

## Keystone reproducibility

`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs` **cannot**
see this class, for two compounding reasons:

- The keystone's only MCP wiring is `fake_mcp_module::register_fake_mcp`
  (`crates/holon-integration-tests/src/test_environment.rs:438,939`), which
  exists as a concurrent-DDL race stressor, not as a sidecar-driven
  `McpSyncEngine`. No `ToolSync`/`ResourceSync` strategy ever runs, so the
  cursorless branch is never taken.
- There is no enable-integration transition in the catalog, so the state
  "N integrations enabled" is not generatable.

Additionally, the one provider that IS wired headlessly is `orgmode` — precisely
the provider that behaves correctly. A keystone rendering this section would
have looked right.

Prod/test parity work that would close it: project integration enablement +
connection status into a queryable table, then assert in the keystone that the
discovery section's rows equal the enabled set.

## Remedy

**OPEN — blocked on a Martin ruling.** The mechanism is settled; the target
behaviour is not, and the options differ in cost and in what they promise the
user. See the lane report for the three options
(enabled-set / connected-set / enabled-with-status) and the recommendation.

No code change landed in this lane. Recording the escape ahead of the fix, per
the bug-gap-triage discipline.
