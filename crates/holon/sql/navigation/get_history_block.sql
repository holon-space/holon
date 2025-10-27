-- Block id of one navigation_history row by id (the cursor's current target).
-- Used by `focus` to detect an idempotent re-focus: if the row the cursor
-- already points at targets the same block, focus is a no-op (no new row).
SELECT block_id FROM navigation_history WHERE id = $current_id
