INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)