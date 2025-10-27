CREATE TABLE IF NOT EXISTS block (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    depth INTEGER NOT NULL DEFAULT 0,
    sort_key TEXT NOT NULL DEFAULT 'a0',
    content TEXT NOT NULL DEFAULT '',
    content_type TEXT NOT NULL DEFAULT 'text',
    source_language TEXT,
    source_name TEXT,
    name TEXT,
    properties TEXT,
    marks TEXT,
    collapsed INTEGER NOT NULL DEFAULT 0,
    completed INTEGER NOT NULL DEFAULT 0,
    block_type TEXT NOT NULL DEFAULT 'text',
    created_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0,
    _change_origin TEXT
);

CREATE INDEX IF NOT EXISTS idx_block_parent_id ON block(parent_id);

-- Document blocks have unique (parent_id, name). Prevents duplicate document
-- creation when concurrent on_file_changed calls race through get_or_create.
CREATE UNIQUE INDEX IF NOT EXISTS idx_block_document_unique
    ON block(parent_id, name) WHERE name IS NOT NULL;
