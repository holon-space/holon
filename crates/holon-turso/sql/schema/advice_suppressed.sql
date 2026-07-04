CREATE TABLE IF NOT EXISTS advice_suppressed (
    anchor_id TEXT NOT NULL,
    lesson_id TEXT NOT NULL,
    PRIMARY KEY (anchor_id, lesson_id),
    -- Source integrity only (`anchor_id` is created in the same transaction as
    -- its edges). The TARGET (`lesson_id`) is a SOFT cross-reference — a lesson
    -- block defined in a separate advice-rules seed / another file, possibly not
    -- yet ingested — and is DELIBERATELY unconstrained. A FK on `lesson_id` would
    -- fail the anchor block's create transaction at COMMIT and abort the WHOLE
    -- file ingest (same data-loss class as `block_requires.required_id`; dogfood
    -- 2026-07-10). The suppression is consumed as an anti-join, which tolerates a
    -- dangling lesson target.
    FOREIGN KEY (anchor_id) REFERENCES block_raw(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_advice_suppressed_lesson ON advice_suppressed(lesson_id);
