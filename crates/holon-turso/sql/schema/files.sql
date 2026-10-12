CREATE TABLE IF NOT EXISTS file (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent_id TEXT NOT NULL,
    content_hash TEXT NOT NULL DEFAULT '',
    document_id TEXT,
    -- JSON array of the block ids a read-only-format file declared at the
    -- ingest that stamped `content_hash`; NULL for a writable file. Written
    -- and read by raw SQL only (like `_change_origin`) — it is the file-sync
    -- controller's state, not a field of the `File` entity, and a boot that
    -- skips the ingest reads its read-only membership from here.
    read_only_blocks TEXT,
    -- Hash of this file's MERGE BASE — the bytes the write-back last put in
    -- it, kept device-local in `<vault>/.holon/bases/` because the base must
    -- survive a vault with no DB and must never sync to another device. A
    -- cold boot diffs the disk against that base; without it the base comes
    -- from the store, and a file left behind its store reads as an external
    -- removal of the store's own unwritten edit. The base is used ONLY when
    -- its bytes hash to this value. Written and read by raw SQL only, like
    -- `read_only_blocks`. Empty when Holon never wrote the file's bytes.
    base_hash TEXT NOT NULL DEFAULT '',
    properties TEXT,
    property_kinds TEXT,
    _change_origin TEXT
);

CREATE INDEX IF NOT EXISTS idx_file_parent_id ON file(parent_id);

CREATE INDEX IF NOT EXISTS idx_file_document_id ON file(document_id);
