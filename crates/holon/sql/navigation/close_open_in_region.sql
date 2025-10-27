-- Soft-close every currently open navigation_history row in a region.
-- Used by `focus_replace`: before inserting the new focus, close the prior
-- open row(s) so focus_roots converges to "exactly one open row per main-panel
-- region" without losing the back/forward history trail.
UPDATE navigation_history
SET closed_at = datetime('now')
WHERE region = $region AND closed_at IS NULL
