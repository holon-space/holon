-- The base block table. Reads should normally go through the `block`
-- matview (synthesized in schema_modules.rs from the EdgeFieldDescriptor
-- registry) which hydrates the edge fields from the junction tables.
-- Writes go to this table.
CREATE TABLE IF NOT EXISTS block_raw (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    depth INTEGER NOT NULL DEFAULT 0,
    sort_key TEXT NOT NULL DEFAULT 'A0',
    content TEXT NOT NULL DEFAULT '',
    content_type TEXT NOT NULL DEFAULT 'text',
    source_language TEXT,
    source_name TEXT,
    properties TEXT,
    marks TEXT,
    collapsed INTEGER NOT NULL DEFAULT 0,
    completed INTEGER NOT NULL DEFAULT 0,
    block_type TEXT NOT NULL DEFAULT 'text',
    created_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0,
    _change_origin TEXT
);

CREATE INDEX IF NOT EXISTS idx_block_raw_parent_id ON block_raw(parent_id);
