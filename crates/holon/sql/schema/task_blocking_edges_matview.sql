SELECT
    tb.blocked_id  AS task_id,
    tb.blocker_id  AS blocker_id,
    b.content      AS blocker_content
FROM task_blockers tb
JOIN block b ON b.id = tb.blocker_id
