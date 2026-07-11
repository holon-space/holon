-- block_links: link junction derived from block.marks Link spans (links increment 2).
-- Written at the SQL write boundary in the SAME transaction as the block row and
-- the other junctions (block_tags/block_requires/advice_suppressed).
--
-- SOFT targets by design: `target` may name a page that does not exist yet
-- (lazy page creation — never mint placeholders), so there is NO foreign key
-- on target/resolved_id. source_block_id also carries no FK (the writer
-- deletes rows explicitly on block delete) — see the deferred-FK autocommit
-- wart: junction FKs cost more than they protect here.
--
-- kind: 'page' (wiki-name target, `target` = name or parent/leaf chain),
--       'block' (id-form target, `target` = schemed URI),
--       'tag'   (tag: scheme target).
-- resolved_id: the target's block id once resolution succeeds; NULL = dangling.

CREATE TABLE IF NOT EXISTS block_links (
    source_block_id TEXT NOT NULL,
    target TEXT NOT NULL,
    kind TEXT NOT NULL,
    resolved_id TEXT,
    PRIMARY KEY (source_block_id, target, kind)
);

CREATE INDEX IF NOT EXISTS idx_block_links_source ON block_links(source_block_id);
CREATE INDEX IF NOT EXISTS idx_block_links_resolved ON block_links(resolved_id);
CREATE INDEX IF NOT EXISTS idx_block_links_kind_target ON block_links(kind, target);
