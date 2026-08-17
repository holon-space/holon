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
--   `enabled_state` — the SAME decision as the toggle's state word ('on'/'off')
--   `config_status` — has the one-time credential setup run (store)
--   `status`        — how far the boot connect got (integration registry)
-- The design defers the third (§8 R9) and notes the table takes the extra
-- column without disturbing anything else; the discovery surface wants it, so
-- it is here. Only the projector writes the first two; only the registry writes
-- the third.
--
-- `enabled_state` is the one fact twice, deliberately: `state_toggle` cycles a
-- WORD, the section is a `live_query`, and SQL is the only language between the
-- mirror and the row. Deriving it with a `CASE` in the section instead put a
-- view CREATE inside every interaction window (+1 ddl, +2 reads on PinBlock,
-- measured 2026-08-18) — the projector, which is the sole writer of both, is
-- the cheaper and more honest place for it. Same reasoning as `status`.
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
    enabled_state TEXT NOT NULL,
    status TEXT NOT NULL,
    config_status TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    _change_origin TEXT
);
