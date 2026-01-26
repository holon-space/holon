CREATE TABLE IF NOT EXISTS task_blockers (
    blocked_id TEXT NOT NULL,
    blocker_id TEXT NOT NULL,
    PRIMARY KEY (blocked_id, blocker_id),
    FOREIGN KEY (blocked_id) REFERENCES block(id) ON DELETE CASCADE,
    FOREIGN KEY (blocker_id) REFERENCES block(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_task_blockers_blocker ON task_blockers(blocker_id);
