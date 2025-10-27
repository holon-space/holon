-- Drop closed (soft-deleted) history rows at or below the retention
-- threshold for the region. Open rows and pins are never touched; the
-- focus_roots matview only consumes open rows, so this is invisible to it.
DELETE FROM navigation_history
WHERE region = $region AND closed_at IS NOT NULL AND id <= $threshold_id
