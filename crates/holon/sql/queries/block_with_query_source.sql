-- Loads a block together with its query source child and optional render source sibling.
-- The {query_langs} placeholder is filled at compile time with QueryLanguage::sql_in_list().
SELECT
    b.id,
    b.parent_id,
    b.content,
    b.properties,
    query_src.content AS query_source,
    query_src.source_language AS query_language,
    render_src.content AS render_source
-- Read from `block_raw` (the writable base table), not the `block`
-- matview. None of the projected columns require the matview's
-- `tags`/`blocked_by` hydration, and reading the matview during a
-- concurrent CDC cycle has been observed to return empty for blocks
-- whose query-source children are still mid-propagation
-- (intermittent inv10d failure: `render_entity` falls through to
-- `render_leaf_block`, root widget renders as `live_block(self_id)`
-- instead of the parsed `tree(...)`). See devlog/2026-05-05-110311.md.
FROM block_raw b
INNER JOIN block_raw query_src ON query_src.parent_id = b.id
    AND query_src.content_type = 'source'
    AND query_src.source_language IN {query_langs}
LEFT JOIN block_raw render_src ON render_src.parent_id = b.id
    AND render_src.content_type = 'source'
    AND render_src.source_language = 'render'
WHERE b.id = $block_id
LIMIT 1
