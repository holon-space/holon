-- Nearest still-open real tab to the RIGHT (higher insertion id) of a closed
-- tab. Cursor-follow falls back to this when no left neighbor exists (the
-- closed tab was the leftmost). `block_id IS NOT NULL` skips home rows; run
-- AFTER the close so `closed_at IS NULL` already excludes the closed row.
SELECT id FROM navigation_history
WHERE region = $region AND closed_at IS NULL AND block_id IS NOT NULL
  AND id > $history_id
ORDER BY id ASC LIMIT 1
