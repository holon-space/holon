-- Turso IVM Bug Reproducer: JOIN with nested materialized views
--
-- BUG: When a materialized view JOINs with another materialized view that itself
-- has a JOIN, inserting into the base table causes a panic:
--   assertion failed: self.current_page >= 0
--   at core/storage/btree.rs
--
-- This reproducer needs to be run in the full holon environment with CDC enabled
-- to trigger the panic. The issue does not reproduce in standalone tests.

-- Setup base tables
CREATE TABLE blocks (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    content TEXT,
    content_type TEXT,
    properties TEXT
);

CREATE TABLE navigation_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    region TEXT NOT NULL,
    block_id TEXT,
    timestamp TEXT DEFAULT (datetime('now'))
);

CREATE TABLE navigation_cursor (
    region TEXT PRIMARY KEY,
    history_id INTEGER REFERENCES navigation_history(id)
);

INSERT INTO navigation_cursor (region, history_id) VALUES ('main', NULL);

-- Create first matview with JOIN (this is the problematic one)
CREATE MATERIALIZED VIEW current_focus AS
SELECT nc.region, nh.block_id, nh.timestamp
FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id;

-- Create second matview that JOINs with the first (nested JOIN)
CREATE MATERIALIZED VIEW watch_view AS
SELECT blocks.id, blocks.parent_id, blocks.content
FROM blocks
INNER JOIN current_focus AS cf ON blocks.parent_id = cf.block_id
WHERE cf.region = 'main';

-- This INSERT triggers the panic when CDC is enabled
BEGIN IMMEDIATE;
INSERT INTO blocks (id, parent_id, content) VALUES ('block-1', 'root', 'Hello');
COMMIT;

-- Expected panic during COMMIT:
--   assertion failed: self.current_page >= 0
--   at turso_core::storage::btree::PageStack::top
--   via turso_core::incremental::join_operator::JoinOperator::commit

-- KNOWN LIMITATIONS (documented):
-- 1. No literal values in JOIN conditions
-- 2. No subqueries in JOIN clauses
-- 3. No LEFT/RIGHT OUTER JOINs
-- 4. [NEW] Nested matview JOINs may panic on insert when CDC is enabled
