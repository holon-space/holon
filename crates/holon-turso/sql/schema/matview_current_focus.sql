SELECT
    nc.region,
    nh.block_id,
    nh.timestamp
FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id
