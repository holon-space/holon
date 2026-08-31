-- The base block table. Reads should normally go through the `block`
-- matview (synthesized in schema_modules.rs from the EdgeFieldDescriptor
-- registry) which hydrates the edge fields from the junction tables.
-- Writes go to this table.
CREATE TABLE IF NOT EXISTS block_raw (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    sort_key TEXT NOT NULL DEFAULT 'A0',
    content TEXT NOT NULL DEFAULT '',
    content_type TEXT NOT NULL DEFAULT 'text',
    source_language TEXT,
    source_name TEXT,
    properties TEXT,
    -- Per-key kind map for `properties`, holding an entry only where the JSON
    -- form is ambiguous (DateTime looks like a string, Json like an object).
    -- NULL means every key reads back at its JSON-evident kind.
    property_kinds TEXT,
    marks TEXT,
    collapsed INTEGER NOT NULL DEFAULT 0,
    widget_only INTEGER NOT NULL DEFAULT 0,
    completed INTEGER NOT NULL DEFAULT 0,
    block_type TEXT NOT NULL DEFAULT 'text',
    created_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0,
    _change_origin TEXT,
    -- Monotonic per-write ordering token (holon_api::write_seq). Ordering-only,
    -- stamped by the editor on each content write so the gpui editor can drop
    -- stale/reordered CDC echoes of earlier keystrokes. Default 0 = never
    -- editor-written; every editor write is > 0.
    write_seq INTEGER NOT NULL DEFAULT 0,
    -- Every block's parent must be a real row in this table. Roots reference
    -- the self-parented `sentinel:no_parent` row (seeded in CoreSchemaModule).
    -- DEFERRABLE INITIALLY DEFERRED so a batch/consolidator transaction that
    -- inserts parent + child (in either order) is only checked at COMMIT.
    FOREIGN KEY (parent_id) REFERENCES block_raw(id) DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX IF NOT EXISTS idx_block_raw_parent_id ON block_raw(parent_id);
