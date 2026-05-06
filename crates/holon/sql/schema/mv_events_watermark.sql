-- Global watermark: max created_at across all events.
-- CDC fires on event inserts (events table is append-only in practice).
CREATE MATERIALIZED VIEW IF NOT EXISTS mv_events_global_watermark AS
SELECT MAX(created_at) AS ts
FROM events
WHERE id = id;

-- Per-consumer ack watermark: max acked_at per consumer.
-- event_acks is insert-only, so the matview output row for each consumer is
-- updated (or inserted on first ack) but never delete+inserted from ack churn.
CREATE MATERIALIZED VIEW IF NOT EXISTS mv_event_acks_watermark AS
SELECT consumer, MAX(acked_at) AS ts
FROM event_acks
GROUP BY consumer;
