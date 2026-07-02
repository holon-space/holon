-- The `block` matview: hydrates the block_raw rows with the
-- edge-typed fields (`tags`, `requires`) reconstructed from the
-- junction tables. Every consumer that wants a hydrated row reads
-- from here; raw structural reads/writes target `block_raw`.
--
-- Shape verified by the chained-matview preflight in
-- bigdata/turso/bugs/holon_block_hydration_repro.sql (CHAIN_* sections,
-- 2026-05-05). G0 + G1 fixes in Turso make the dual-LEFT + json_group_array
-- + GROUP BY pattern produce all rows including those with empty
-- junctions, and CDC propagates correctly through DELETE.
--
-- All 17 columns of block_raw are projected so downstream readers
-- (block_with_path, block_requirement_edges, GQL/PRQL-generated
-- watch_view_*) see the same row shape they did before the table
-- was renamed.
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
    b.completed,
    b.block_type,
    b.created_at,
    b.updated_at,
    b._change_origin,
    COALESCE(json_group_array(bt.tag)         FILTER (WHERE bt.tag         IS NOT NULL), '[]') AS tags,
    COALESCE(json_group_array(br.required_id) FILTER (WHERE br.required_id IS NOT NULL), '[]') AS requires
FROM block_raw b
LEFT OUTER JOIN block_tags     bt ON bt.block_id = b.id
LEFT OUTER JOIN block_requires br ON br.block_id = b.id
GROUP BY
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
    b.completed,
    b.block_type,
    b.created_at,
    b.updated_at,
    b._change_origin
