CREATE TABLE IF NOT EXISTS block_requires (
    block_id TEXT NOT NULL,
    required_id TEXT NOT NULL,
    PRIMARY KEY (block_id, required_id),
    -- Source integrity only: the requiring block must exist (it is created in
    -- the same transaction as its edges). The TARGET (`required_id`) is a SOFT
    -- cross-reference and is DELIBERATELY unconstrained — a `:REQUIRES:` /
    -- `:BLOCKED-BY:` edge legitimately points forward within a file (a block
    -- parsed later in the same scan) or across files (a block in another org
    -- file scanned later, or never). A FK on `required_id` would fail the
    -- block's create transaction at COMMIT and abort the WHOLE file ingest
    -- (dogfood 2026-07-10 region-writeback data loss). Dangling targets are a
    -- normal, representable state; `block_requirement_edges_matview` INNER-JOINs
    -- on the target, so an absent target simply yields no matview row.
    FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_block_requires_required ON block_requires(required_id);
