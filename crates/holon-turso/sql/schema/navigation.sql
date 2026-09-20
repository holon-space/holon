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

-- Which live watch is looking at which subtree root. This is what makes ONE
-- descendant matview serve every watch: the root enters the view as DATA (a
-- row joined in) instead of as a literal baked into the view's SQL, so the
-- view's text — and therefore its content-addressed name — no longer varies
-- per block. Without it, every first visit to a block mints its own
-- O(vault) matview on the interaction path.
--
-- Session-scoped UI state, Turso-only: it never enters Loro or org, and boot
-- TRUNCATES it before this session registers its watches. A row that outlives
-- its watch has no owner and would keep a subtree incrementally maintained
-- forever.
-- ONE row per PLACE, shared by every watch looking at it. A row per WATCH was
-- measured and rejected: both ways of collapsing the duplicates it creates
-- break production. `SELECT DISTINCT` in the seed is accepted by the planner
-- and IS maintained for a simple shape, but NOT for this one — the real root
-- view empties out against its own recompute
-- (`inv-matview-consistent-with-recompute`, lane-logs/v10-hand-authored.log,
-- isolated to the DISTINCT by lane-logs/v11-hand-authored-nodistinct.log). A
-- `GROUP BY` key relation the views join is not maintained either: the engine
-- drops the whole group on a partial delete
-- (`holon-turso/tests/watch_context_dedup_maintenance.rs`). Without any
-- dedup, two watches on one place duplicate every row and the panel drops
-- them. So the row is a PLACE's membership, and WHICH watches hold it is
-- state of the engine that owns this database (`BackendEngine::watch_places`,
-- per engine — never a process-global, which two engines on two databases
-- would share).
--
-- `nonce` names this incarnation of the row. The release a departing last
-- owner issues is spawned, so it can arrive after a successor watch re-took
-- the place; qualified by the nonce it matches nothing then, instead of
-- deleting a row a live watch depends on.
--
-- `kind` is the view shape the row belongs to, so the descendant view
-- maintains no closure for a watch that only wants the single block.
CREATE TABLE IF NOT EXISTS watch_context (
    watch_key TEXT PRIMARY KEY,
    context_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    nonce TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_watch_context_context_id
ON watch_context(context_id);
