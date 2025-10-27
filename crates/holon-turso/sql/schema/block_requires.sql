CREATE TABLE IF NOT EXISTS block_requires (
    block_id TEXT NOT NULL,
    required_id TEXT NOT NULL,
    PRIMARY KEY (block_id, required_id),
    FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE,
    FOREIGN KEY (required_id) REFERENCES block_raw(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_block_requires_required ON block_requires(required_id);
