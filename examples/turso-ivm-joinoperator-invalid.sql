-- Turso IVM JoinOperator Invalid State Reproducer
--
-- BUG: JoinOperator::commit panics with "Invalid state reached" when:
-- 1. A materialized view with a JOIN exists
-- 2. Data is inserted into a DIFFERENT table that doesn't affect the JOIN view
-- 3. apply_view_deltas is called for ALL views during commit
-- 4. The JoinOperator for the JOIN view is in Invalid state
--
-- Expected panic:
-- [JoinOperator::commit] Invalid state reached! previous_state=Invalid,
-- left_storage_id=..., right_storage_id=..., input_deltas: left_changes=..., right_changes=0
--
-- Run with turso CLI or libturso test

-- STEP 1: Create events table with indexes
CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    origin TEXT NOT NULL,
    status TEXT DEFAULT 'confirmed',
    payload TEXT NOT NULL,
    trace_id TEXT,
    command_id TEXT,
    created_at INTEGER NOT NULL,
    processed_by_loro INTEGER DEFAULT 0,
    processed_by_org INTEGER DEFAULT 0,
    processed_by_cache INTEGER DEFAULT 0,
    speculative_id TEXT,
    rejection_reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_events_loro_pending
    ON events(created_at)
    WHERE processed_by_loro = 0 AND origin != 'loro' AND status = 'confirmed';

CREATE INDEX IF NOT EXISTS idx_events_org_pending
    ON events(created_at)
    WHERE processed_by_org = 0 AND origin != 'org' AND status = 'confirmed';

CREATE INDEX IF NOT EXISTS idx_events_cache_pending
    ON events(created_at)
    WHERE processed_by_cache = 0 AND status = 'confirmed';

CREATE INDEX IF NOT EXISTS idx_events_aggregate
    ON events(aggregate_type, aggregate_id, created_at);

CREATE INDEX IF NOT EXISTS idx_events_command
    ON events(command_id)
    WHERE command_id IS NOT NULL;

-- STEP 2: Create matview on events (NO JOIN)
CREATE MATERIALIZED VIEW events_view_block AS
    SELECT * FROM events
    WHERE status = 'confirmed' AND aggregate_type = 'block';

-- STEP 3: Create navigation tables (for the JOIN view)
CREATE TABLE IF NOT EXISTS navigation_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    region TEXT NOT NULL,
    block_id TEXT,
    timestamp TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_navigation_history_region
    ON navigation_history(region);

CREATE TABLE IF NOT EXISTS navigation_cursor (
    region TEXT PRIMARY KEY,
    history_id INTEGER REFERENCES navigation_history(id)
);

-- Initialize cursor with NULL history_id
INSERT INTO navigation_cursor (region, history_id) VALUES ('main', NULL);

-- STEP 4: Create current_focus view (HAS A JOIN!)
DROP VIEW IF EXISTS current_focus;

CREATE MATERIALIZED VIEW current_focus AS
    SELECT
        nc.region,
        nh.block_id,
        nh.timestamp
    FROM navigation_cursor nc
    JOIN navigation_history nh ON nc.history_id = nh.id;

-- STEP 5: INSERT into events table
-- This is where the bug manifests:
-- - events table is NOT part of current_focus view
-- - But apply_view_deltas is called for ALL views during commit
-- - The JoinOperator for current_focus is in Invalid state
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin,
    status, payload, created_at
) VALUES (
    '01KE1Z0Y3YDVGJ6WK0TESTEVT1',
    'block_created',
    'block',
    'd153ac2e-64b6-4b98-8c92-eb82f0c9e123',
    'loro',
    'confirmed',
    '{"type":"block_created","block_id":"test-block"}',
    1735909133978
);
