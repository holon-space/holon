-- Resolves focus targets to the *children* of the navigated-to block.
-- When the user navigates to block X, the main panel should show X's
-- children (and their subtrees), NOT X itself. The GQL query then uses
-- CHILD_OF*0..N from each root to include the child + its descendants.
--
-- NOTE: Joins base tables directly instead of current_focus matview.
-- Turso IVM doesn't reliably cascade changes through chained matviews
-- (matview-on-matview), causing focus_roots to retain stale rows.
SELECT nc.region, nh.block_id, b.id AS root_id
FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id
JOIN block AS b ON b.parent_id = nh.block_id
