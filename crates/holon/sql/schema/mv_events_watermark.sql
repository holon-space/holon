CREATE MATERIALIZED VIEW IF NOT EXISTS mv_events_watermark AS
SELECT id, created_at, processed_by_loro, processed_by_org, processed_by_cache
FROM events
WHERE id = id
