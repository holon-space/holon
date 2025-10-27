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
