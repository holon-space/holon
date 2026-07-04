-- One-read lookup for `close`: the region owning a navigation_history row plus
-- that region's CURRENT cursor target. `close` carries only the row handle, so
-- this resolves both the region (to scope the neighbor search) and whether the
-- closed row IS the active tab (cursor-follow only fires then) in a single
-- round-trip. `cursor_id` is NULL when the region has no cursor row yet.
SELECT nh.region AS region, nc.history_id AS cursor_id
FROM navigation_history nh
LEFT JOIN navigation_cursor nc ON nc.region = nh.region
WHERE nh.id = $history_id
