CREATE TABLE IF NOT EXISTS document (
    id TEXT PRIMARY KEY NOT NULL,
    parent_id TEXT NOT NULL,
    name TEXT NOT NULL,
    sort_key TEXT NOT NULL,
    properties TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    _change_origin TEXT
);

CREATE INDEX IF NOT EXISTS idx_document_parent_id ON document(parent_id);

CREATE INDEX IF NOT EXISTS idx_document_name ON document(name);
