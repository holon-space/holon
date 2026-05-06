-- Refresh the timestamp of an existing open pin so it sorts to the top of
-- the right sidebar. `focus_pin` calls this first; if rowsAffected = 0, no
-- existing pin → insert_history.sql runs instead. The pair gives move-to-top
-- dedup with one round-trip per click in the common case.
UPDATE navigation_history
SET timestamp = datetime('now')
WHERE region = $region AND block_id = $block_id AND closed_at IS NULL
