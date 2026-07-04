-- journal_feed: FEED layer of the journal-feed chain — a matview chained on
-- the `journal_day_pages` detection matview (matview-on-matview; supported on
-- the pinned Turso rev, proven by test_ivm_chained_matview_reopen and by the
-- production `block_with_path` / `focus_roots` chains).
--
-- Adds the feed projection: `expand_default = 1` so `render_entity()` shows
-- each day's children inline. Ordering (`content DESC`) belongs to the read
-- query, not the matview (matviews are unordered sets — same convention as
-- `automations_journal`).
--
-- This relation is deliberately UNBOUNDED: a `LIMIT` here (or in the read) is
-- refused by the windowing contract in `util.rs:43-48` — a windowed snapshot
-- cannot agree with an unbounded CDC stream, and `strip_order_by` discards a
-- trailing LIMIT anyway. Feed windowing lives in the VIEW layer, downstream of
-- the stream (ruling 2026-08-11; see docs/Plans/JournalFeed-2026-07-18.md §3).
SELECT
    id,
    parent_id,
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
