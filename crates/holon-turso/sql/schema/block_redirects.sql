-- Merge redirects: `from_id` was folded into `to_id` by `merge_blocks`, so
-- `from_id` must keep resolving. DERIVED, not authoritative: the replicated
-- fact is the surviving block's `merged_from` property (main Loro doc), and
-- these rows are re-derived from it at the SQL write boundary — the same
-- shape as `block_links` deriving from `marks`.
--
-- `from_id` is the PK because an id can only ever be merged away once; a
-- second merge of the same id is refused rather than overwritten.
CREATE TABLE IF NOT EXISTS block_redirects (
    from_id TEXT PRIMARY KEY NOT NULL,
    to_id TEXT NOT NULL,
    merged_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_block_redirects_to_id ON block_redirects(to_id);
