CREATE TABLE IF NOT EXISTS sync_states (
    provider_name TEXT PRIMARY KEY,
    sync_token TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
