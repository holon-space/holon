-- `Block::try_from` REQUIRES every edge column plus the typed scalar columns
-- (`marks`/`collapsed`/`widget_only`) — a read that omits one fails the whole
-- row rather than yielding a block with a silently empty edge set. The `block`
-- matview hydrates all four junction aggregates, so project them verbatim.
SELECT
    id, parent_id, content, content_type, source_language,
    source_name, properties, marks, collapsed, widget_only,
    created_at, updated_at,
    tags, requires, advice_suppressed, contributes_to
FROM block
WHERE json_extract(properties, '$.task_state') IS NOT NULL
   OR json_extract(properties, '$.prototype_for') IS NOT NULL
   OR json_extract(properties, '$.verb_op') IS NOT NULL
   OR json_extract(properties, '$.is_self') = true
ORDER BY parent_id, sort_key
