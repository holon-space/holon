-- block_history: the C2b op/effect history relation (ADR 0024 P8).
-- A DISCLOSED EPHEMERAL CACHE: rebuildable, never authoritative (Layer 3).
-- Schema evolution = drop + recreate (see holon_api::history module docs);
-- HistorySchemaModule drops a stale-shaped table before running this file.
-- schema-version: 2 (sentinel column: op_group)
CREATE TABLE IF NOT EXISTS block_history (
    seq INTEGER PRIMARY KEY,
    entity_name TEXT NOT NULL,
    block_id TEXT NOT NULL,
    op_name TEXT NOT NULL,
    origin TEXT NOT NULL,
    transition_id TEXT,
    session_id TEXT,
    tool_call_id TEXT,
    effect_id TEXT,
    field TEXT,
    old_value TEXT,
    new_value TEXT,
    at_millis INTEGER NOT NULL,
    day TEXT NOT NULL,
    op_group INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_block_history_block ON block_history(block_id);
CREATE INDEX IF NOT EXISTS idx_block_history_session ON block_history(session_id);
CREATE INDEX IF NOT EXISTS idx_block_history_at ON block_history(at_millis);
CREATE INDEX IF NOT EXISTS idx_block_history_day ON block_history(day);
CREATE INDEX IF NOT EXISTS idx_block_history_group ON block_history(op_group);
