-- block_link: extracted [[...]] links from block content
-- Populated by LinkEventSubscriber, not by direct SQL from sync controllers.
-- target_id is entity-type-agnostic (doc:, block:, person:, etc.)

CREATE TABLE IF NOT EXISTS block_link (
    source_block_id TEXT NOT NULL,
    target_raw TEXT NOT NULL,
    target_id TEXT,
    display_text TEXT,
    position INTEGER NOT NULL,
    PRIMARY KEY (source_block_id, position)
);

CREATE INDEX IF NOT EXISTS idx_block_link_source ON block_link(source_block_id);
CREATE INDEX IF NOT EXISTS idx_block_link_target_id ON block_link(target_id);
