-- journal_day_pages: DAY-PAGE DETECTION layer of the journal-feed chain.
-- One row per journal day page — a block tagged `Page` whose parent is the
-- seeded `block:journals` Page. `content` is the date (`YYYY-MM-DD`).
--
-- Chained on the `block` matview (which hydrates tags/requires from the
-- junction tables) JOINed against the `block_tags` base table — the same
-- matview-JOIN-junction shape IVM already maintains for `focus_roots` and
-- `block_requirement_edges`. No fan-out: `block_tags` PK is
-- `(block_id, tag)` and we filter the single `Page` tag, so exactly one row
-- per day page. IVM maintains it O(delta) as day pages are created / edited /
-- deleted.
--
-- Columns are the full `block` matview projection (never `*` — matview DDL
-- enumerates columns, matching every other boot matview) so downstream reads
-- and `render_entity()` see a complete block row.
SELECT
    b.id,
    b.parent_id,
    b.depth,
    b.sort_key,
    b.content,
    b.content_type,
    b.source_language,
    b.source_name,
    b.properties,
    b.marks,
    b.collapsed,
    b.widget_only,
    b.completed,
    b.block_type,
    b.created_at,
    b.updated_at,
    b._change_origin,
    b.write_seq,
    b.tags,
    b.requires,
    b.advice_suppressed
FROM block b
JOIN block_tags bt ON bt.block_id = b.id AND bt.tag = 'Page'
WHERE b.parent_id = 'block:journals'
