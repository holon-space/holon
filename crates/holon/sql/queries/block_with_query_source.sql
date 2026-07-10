-- Loads a block together with its query source child and optional render source sibling.
-- The {query_langs} placeholder is filled at compile time with QueryLanguage::sql_in_list().
SELECT
    b.id,
    b.parent_id,
    b.content,
    b.properties,
    query_src.content AS query_source,
    query_src.source_language AS query_language,
    render_src.content AS render_source,
    -- WP3 / C-revised ruling: is this query-source child actually a rule's
    -- TRIGGER (a sibling of a `holon_rule`/`action` head)? Such a block is
    -- program machinery — the action watcher is its sole evaluator — and must
    -- NEVER be compiled into a display matview. `render_entity` fails loud on a
    -- truthy flag instead of running `query_and_watch` on the trigger.
    EXISTS (
        SELECT 1 FROM block_raw rh
        WHERE rh.parent_id = query_src.parent_id
            AND rh.content_type = 'source'
            AND (rh.source_language = 'holon_rule' OR rh.source_language = 'action')
    ) AS query_src_is_rule_trigger
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
