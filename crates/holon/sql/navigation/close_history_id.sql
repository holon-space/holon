-- Soft-close one specific navigation_history row by id. Used by the
-- right sidebar X button — `close(history_id)` from focus_roots.history_id.
UPDATE navigation_history
SET closed_at = datetime('now')
WHERE id = $history_id
