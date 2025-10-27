CREATE TABLE IF NOT EXISTS sync_states (
    provider_name TEXT PRIMARY KEY NOT NULL,
    sync_token TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    _change_origin TEXT
);
