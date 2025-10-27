CREATE TABLE IF NOT EXISTS file (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent_id TEXT NOT NULL,
    content_hash TEXT NOT NULL DEFAULT '',
    document_id TEXT,
    _change_origin TEXT
);

CREATE INDEX IF NOT EXISTS idx_file_parent_id ON file(parent_id);

CREATE INDEX IF NOT EXISTS idx_file_document_id ON file(document_id);
