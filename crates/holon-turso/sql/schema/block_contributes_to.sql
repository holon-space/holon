CREATE TABLE IF NOT EXISTS block_contributes_to (
    block_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    PRIMARY KEY (block_id, target_id),
    -- Source integrity only, exactly as `block_requires`: the contributing
    -- block is created in the same transaction as its edges, while the TARGET
    -- is a SOFT cross-reference — a Compass goal or mission authored in another
    -- file that may be scanned later, or never. A FK on `target_id` would fail
    -- the block's create transaction at COMMIT and abort the whole file ingest.
    FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_block_contributes_to_target ON block_contributes_to(target_id);
