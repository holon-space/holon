CREATE TABLE IF NOT EXISTS advice_suppressed (
    anchor_id TEXT NOT NULL,
    lesson_id TEXT NOT NULL,
    PRIMARY KEY (anchor_id, lesson_id),
    FOREIGN KEY (anchor_id) REFERENCES block_raw(id) ON DELETE CASCADE,
    FOREIGN KEY (lesson_id) REFERENCES block_raw(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_advice_suppressed_lesson ON advice_suppressed(lesson_id);
