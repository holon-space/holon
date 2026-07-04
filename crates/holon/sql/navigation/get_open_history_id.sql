-- Lowest open navigation_history row id for a (region, block_id), or none.
-- `open_tab` uses this to dedup: if the block is already an open tab, it
-- activates that existing row instead of inserting a duplicate. ORDER BY id
-- ASC picks the original (stable insertion order) if several ever coexist.
SELECT id FROM navigation_history
WHERE region = $region AND block_id = $block_id AND closed_at IS NULL
ORDER BY id ASC LIMIT 1
