-- Nearest still-open real tab to the LEFT (lower insertion id) of a closed tab.
-- Cursor-follow prefers the left neighbor (the tab the user most recently had
-- before the closed one in stable insertion order), matching common editors.
-- `block_id IS NOT NULL` skips home rows; run AFTER the close so `closed_at IS
-- NULL` already excludes the just-closed row.
SELECT id FROM navigation_history
WHERE region = $region AND closed_at IS NULL AND block_id IS NOT NULL
  AND id < $history_id
ORDER BY id DESC LIMIT 1
