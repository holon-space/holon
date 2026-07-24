CREATE TABLE IF NOT EXISTS navigation_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    region TEXT NOT NULL,
    block_id TEXT,
    timestamp TEXT DEFAULT (datetime('now')),
    -- DISPLAY FLAG (not a departure timestamp): `NULL` = open (row appears in
    -- the focus_roots matview); non-NULL = closed (omitted from focus_roots).
    -- The value is a bookkeeping `datetime('now')` only so the flag is a single
    -- column; nothing reads it as history metadata. Rows are NEVER hard-deleted
    -- on navigation, so back/forward history is retained regardless of this flag.
    -- focus_replace closes the prior open row before inserting a new one;
    -- focus_pin updates the existing open row's timestamp instead of inserting
    -- (move-to-top dedup); close(history_id) closes one specific row (sidebar X).
    -- INVARIANT (write-side, NavigationProvider): navigation_cursor always
    -- points at an OPEN row (closed_at IS NULL) or has no/NULL cursor —
    -- go_back/go_forward RE-OPEN their target (flip closed_at → NULL) and close
    -- the departed row in one transaction so the cursor never lands on a closed
    -- row (which would blank the main panel via the focus_roots join).
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
