CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    origin TEXT NOT NULL,
    status TEXT DEFAULT 'confirmed',
    payload TEXT NOT NULL,
    trace_id TEXT,
    span_id TEXT,
    trace_flags INTEGER DEFAULT 0,
    command_id TEXT,
    created_at INTEGER NOT NULL,
    speculative_id TEXT,
    rejection_reason TEXT
);

-- Per-consumer ack table. Insert-only: marking an event processed never
-- mutates the events row, so it does not perturb the `events_view_*` or
-- `mv_events_*_watermark` matviews. Each ack is a single INSERT and one
-- matview-insert CDC delta (no delete+insert pair).
CREATE TABLE IF NOT EXISTS event_acks (
    event_id TEXT NOT NULL,
    consumer TEXT NOT NULL,
    acked_at INTEGER NOT NULL,
    PRIMARY KEY (event_id, consumer)
);
CREATE INDEX IF NOT EXISTS idx_event_acks_consumer ON event_acks(consumer);
