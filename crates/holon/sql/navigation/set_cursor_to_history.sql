-- Point a region's cursor at an already-open navigation_history row (tab
-- switch / `activate`). Moves ONLY the cursor: inserts no row, closes no row,
-- and does not reorder the open set, so the open tabs keep their stable
-- insertion order (and per-tab scroll survives). Upsert because a region's
-- cursor row may not exist yet on first activate.
INSERT OR REPLACE INTO navigation_cursor (region, history_id)
VALUES ($region, $history_id)
