-- Queryable mirror of the integration enablement store (D5.b §4.1a).
--
-- `IntegrationConfigStore` (filesystem `.state.toml` files) is the authority;
-- this table exists only so a `live_query` can read integration state, which no
-- signal-based store can serve. `IntegrationStateProjector` is its SOLE writer
-- for the store-derived columns and re-derives them from the store, so the
-- mirror is repairable at any time and never becomes a second source of truth.
--
-- One row per BUNDLED provider, enabled or not: the presence axis in full, so a
-- disabled provider is `enabled = 0` rather than an absence indistinguishable
-- from "not projected yet". The discovery reading is `WHERE enabled = 1`.
--
-- THREE axes, deliberately separate:
--   `enabled`       — the stored decision (store)
--   `config_status` — has the one-time credential setup run (store)
--   `status`        — how far the boot connect got (integration registry)
-- The design defers the third (§8 R9) and notes the table takes the extra
-- column without disturbing anything else; the discovery surface wants it, so
-- it is here. Only the projector writes the first two; only the registry writes
-- the third.
--
-- Deliberately ABSENT: no sync_token, no cursor, no credential reference.
-- `sync_states` keeps its own job; conflating the two is the defect this table
-- replaces (bugfunnel 2026-08-18-integrations-discovery-section-lists-only-orgmode).
-- `config_status` is the DISPLAY enum only — never `Configuration`, which
-- carries credential locations.
CREATE TABLE IF NOT EXISTS integration_state (
    id TEXT PRIMARY KEY NOT NULL,
    provider_name TEXT NOT NULL,
    enabled INTEGER NOT NULL,
    status TEXT NOT NULL,
    config_status TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    _change_origin TEXT
);
