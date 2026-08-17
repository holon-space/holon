---
id: 2026-07-26-permanent-wedge-real-vault-scale-ingesting
date: 2026-07-26
gap: COVERAGE
secondary: COVERAGE
status: OPEN
summary: >-
  P0 PERMANENT WEDGE at real-vault scale: ingesting the real
  `Projects/Holon.org` (2.1 MB, 15,763 `:ID:` headlines, 8 levels deep, zero
  links) pins the single Turso actor at 100% CPU forever — UI dead, MCP hangs,
  the remaining ~98 files never ingest (`sample`: 4067/4067 frames in
  `run_actor → handle_query → Program::step`). Root cause = the document-read
  recursive CTE in `CacheBlockReader::get_blocks`
  (crates/holon-app/src/turso_seams.rs:231): `EXPLAIN QUERY PLAN` shows `SCAN
  block_raw` for the recursive arm's `b.parent_id = d.id` (and for the final
  `JOIN descendants`), NOT `SEARCH … USING INDEX idx_block_raw_parent_id` —
  Turso's planner indexes the same equality fine as a standalone predicate but
  not as a recursive-CTE join, so the walk is O(N²). Measured (debug,
  `single_large_document_scale`): 501→708ms, 1001→2533ms, 2001→9890ms
  (exponent ≈1.9); the same rows via a flat `SELECT id,parent_id FROM
  block_raw` are 1.9/3.5/6.1ms (linear, ~1600× faster at N=2001). Extrapolated
  to 15,763 blocks: ~10 min per invocation in debug, ~1 min in release — and
  the query is re-issued on EVERY block change of the document, so it never
  converges. SUPERLINEAR, not non-terminating (flat RSS + zero DB growth =
  pure scan, no accumulation). Amplification: Turso runs as ONE actor, so this
  one query starves the whole process.
source_line: 1107
---

## Bug

P0 PERMANENT WEDGE at real-vault scale: ingesting the real
`Projects/Holon.org` (2.1 MB, 15,763 `:ID:` headlines, 8 levels deep, zero
links) pins the single Turso actor at 100% CPU forever — UI dead, MCP hangs,
the remaining ~98 files never ingest (`sample`: 4067/4067 frames in
`run_actor → handle_query → Program::step`). Root cause = the document-read
recursive CTE in `CacheBlockReader::get_blocks`
(crates/holon-app/src/turso_seams.rs:231): `EXPLAIN QUERY PLAN` shows `SCAN
block_raw` for the recursive arm's `b.parent_id = d.id` (and for the final
`JOIN descendants`), NOT `SEARCH … USING INDEX idx_block_raw_parent_id` —
Turso's planner indexes the same equality fine as a standalone predicate but
not as a recursive-CTE join, so the walk is O(N²). Measured (debug,
`single_large_document_scale`): 501→708ms, 1001→2533ms, 2001→9890ms
(exponent ≈1.9); the same rows via a flat `SELECT id,parent_id FROM
block_raw` are 1.9/3.5/6.1ms (linear, ~1600× faster at N=2001). Extrapolated
to 15,763 blocks: ~10 min per invocation in debug, ~1 min in release — and
the query is re-issued on EVERY block change of the document, so it never
converges. SUPERLINEAR, not non-terminating (flat RSS + zero DB growth =
pure scan, no accumulation). Amplification: Turso runs as ONE actor, so this
one query starves the whole process.

## Missing piece

keystone generators have a hard scale ceiling — no case builds a document
beyond tens of blocks, so no complexity-class defect in any per-document
query can ever surface. Added
`crates/holon-integration-tests/tests/single_large_document_scale.rs`
(env-gated `HOLON_SCALE_BLOCKS`, default 200 = green, ≥1000 = red) asserting
the document read stays ≤0.5 ms/block and printing the prod-vs-flat growth
ladder + EXPLAIN plans.

## Remedy

OPEN (test landed red-for-the-right-reason; fix is an architecture fork —
(a) teach the Turso fork's planner to use indexes for recursive-CTE join
equalities, or (b) drop the Phase-5 SQL-push and read the document with one
flat indexed scan + Rust BFS, plus fix the per-block-change call frequency.
Escalated to Martin.)
