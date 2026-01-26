CREATE TABLE IF NOT EXISTS block_tags (
    block_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (block_id, tag),
    FOREIGN KEY (block_id) REFERENCES block(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_block_tags_tag ON block_tags(tag);
