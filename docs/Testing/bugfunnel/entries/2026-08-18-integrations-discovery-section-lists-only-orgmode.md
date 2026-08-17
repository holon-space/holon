---
id: 2026-08-18-integrations-discovery-section-lists-only-orgmode
date: 2026-08-18
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
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

**FIXED** per Martin's ruling D7.c, built to the shape named in
`docs/Proposals/state-toggle-layout-settings-2026-08-18.md` §4.1a so the D5.b
lane inherits this table unchanged (that design's §8 R11 is exactly the risk of
building it twice).

`IntegrationConfigStore` is now mirrored into a queryable `integration_state`
table (`crates/holon-turso/sql/schema/integration_state.sql`,
`IntegrationStateSchemaModule` in `crates/holon-turso/src/schema_modules.rs`,
`IntegrationStateTables` registered as an eager schema root in
`crates/holon/src/di/schema_providers.rs`). The seeded section queries that
instead of `sync_states`:

```
SELECT provider_name, status FROM integration_state WHERE enabled = 1 ORDER BY provider_name ASC
```

`IntegrationStateProjector` (`crates/holon-app/src/integration_projection.rs`)
is the table's SOLE writer. It re-derives every row from the store on each run
rather than accumulating deltas — the stateful-regrouping law of the
derived-data contract — so a mirror that drifted for any reason re-converges on
the next projection. The row set is every BUNDLED provider, enabled or not, so
the table carries the presence axis in full; the discovery reading is the
`WHERE enabled = 1` above. Boot wires it in `crates/holon-app/src/wiring.rs`,
deliberately there rather than in the lazily-resolved integration registry,
since a container that never touches an integration would otherwise render the
section empty with everything switched on.

The table carries THREE deliberately separate axes: `enabled` (the stored
decision), `config_status` (has the one-time credential setup run —
`unconfigured` | `configured`, the DISPLAY enum only, never `Configuration`
itself, which carries credential LOCATIONS), and `status` (how far the boot
connect got — `Pending` | `Connected` | `Needs auth` | `Unavailable`). Only the
projector writes the first two; only the integration registry writes the third,
from the four outcomes its connect loop already computes
(`crates/holon-app/src/mcp_integrations.rs`).

The design defers the connection axis (§8 R9) while noting the table takes the
extra column without disturbing anything else; the orchestrator ruled it in for
the discovery surface, because switched-on and actually-working are different
questions and collapsing them is what made the old section lie. Re-projection
never clobbers a resolved `status` back to `Pending`, so toggling one
integration cannot make another's column lie.

Oracles (`crates/holon-app/tests/integration_state_projection.rs`), all driving
the sql extracted from the seeded render rather than a restated copy — a test
carrying its own query would have kept passing after the seed drifted, which is
the shape of this escape:

- `seeded_section_lists_exactly_the_enabled_integrations` — THE red. Against the
  `sync_states` seed it returned `[]` for four enabled integrations.
- `disabling_an_integration_removes_it_from_the_section`
- `nothing_enabled_lists_nothing`
- `the_section_carries_each_integrations_boot_status`
- `a_status_for_a_disabled_integration_is_refused` — a status for a provider the
  store never enabled is a wiring bug, and fails loud
- `config_status_tracks_the_stores_configuration_axis`
- `the_mirror_exposes_exactly_the_designed_columns` — pins the column set (§8
  R1), so a later field addition is a failing test rather than a silent
  credential leak into a user-queryable, MCP-readable table.
- `a_drifted_mirror_reconverges_on_the_next_projection` — the convergence
  contract (§8 R4); the case a naive delta-applying projector fails.
- `removing_the_root_layout_reseeds_the_current_section_form` — pins the
  recreate path an existing vault needs.

Two consequences worth naming. `orgmode` no longer appears: it is the vault
itself, always on, with no state file and no enable/disable, and it was only
ever visible because it was the one provider writing a bare provider name into
the cursor table. And per ruling D7b.b there is no migration code — an
already-seeded vault keeps the old section until its root layout is removed,
which is the one-time manual gesture the fix lane documented.
