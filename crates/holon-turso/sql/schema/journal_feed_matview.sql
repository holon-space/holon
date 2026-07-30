-- journal_feed: FEED layer of the journal-feed chain — a matview chained on
-- the `journal_day_pages` detection matview (matview-on-matview; supported on
-- the pinned Turso rev, proven by test_ivm_chained_matview_reopen and by the
-- production `block_with_path` / `focus_roots` chains).
--
-- Adds the feed projection: `expand_default = 1` so `render_entity()` shows
-- each day's children inline. Ordering (`content DESC`) belongs to the read
-- query, not the matview (matviews are unordered sets — same convention as
-- `automations_journal`). This layer is the seam where feed windowing / LIMIT
-- will live (increment 2).
SELECT
    id,
    parent_id,
    depth,
    sort_key,
    content,
    content_type,
    source_language,
    source_name,
    properties,
    marks,
    collapsed,
    widget_only,
    completed,
    block_type,
    created_at,
    updated_at,
    _change_origin,
    write_seq,
    tags,
    requires,
    advice_suppressed,
    1 AS expand_default
FROM journal_day_pages
