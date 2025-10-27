CREATE TABLE IF NOT EXISTS directory (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent_id TEXT NOT NULL,
    depth INTEGER NOT NULL,
    _change_origin TEXT
);

CREATE INDEX IF NOT EXISTS idx_directory_parent_id ON directory(parent_id);
