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

CREATE TABLE IF NOT EXISTS navigation_cursor (
    region TEXT PRIMARY KEY,
    history_id INTEGER REFERENCES navigation_history(id)
);

-- Editor cursor state: tracks which block has text focus and where the cursor is.
-- Separate from navigation_cursor (page-level history). One row per region,
-- replaced on each focus change. Single-row-per-region ensures the CDC matview
-- emits exactly one row, preventing stale cursor positions from overriding clicks.
CREATE TABLE IF NOT EXISTS editor_cursor (
    region TEXT NOT NULL PRIMARY KEY,
    block_id TEXT NOT NULL,
    cursor_offset INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT DEFAULT (datetime('now'))
)
