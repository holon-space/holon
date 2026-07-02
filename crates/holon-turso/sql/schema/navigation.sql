CREATE TABLE IF NOT EXISTS navigation_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    region TEXT NOT NULL,
    block_id TEXT,
    timestamp TEXT DEFAULT (datetime('now')),
    -- Soft-close timestamp. `NULL` = open (in focus_roots matview); set =
    -- closed (omitted from focus_roots, retained for back/forward history).
    -- focus_replace closes the prior open row before inserting a new one;
    -- focus_pin updates the existing open row's timestamp instead of inserting
    -- (move-to-top dedup); close(history_id) closes one specific row (sidebar X).
    closed_at TEXT NULL
);

CREATE INDEX IF NOT EXISTS idx_navigation_history_region
ON navigation_history(region);

-- Editor focus/caret is no longer persisted (pure in-memory UI state, ADR 0010);
-- the `editor_cursor` table and `current_editor_focus` matview were removed.
CREATE TABLE IF NOT EXISTS navigation_cursor (
    region TEXT PRIMARY KEY,
    history_id INTEGER REFERENCES navigation_history(id)
);
