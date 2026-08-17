---
id: 2026-08-12-already-held-clobbers-holder-when-carries
date: 2026-08-12
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A `create` at an ALREADY-HELD `id` clobbers the holder when it carries no
  title
source_line: 723
---

## Bug

(collision-semantics lane, task #8; found by agent adjudication of the
create path; no automated test produced it) **A `create` at an ALREADY-HELD
`id` clobbers the holder when it carries no title** — `prepare_create`'s
unguarded `INSERT … ON CONFLICT(id) DO UPDATE SET <every supplied non-id
column>` lands, because the ADR 0029 D1b gate in `create_row` was
content-gated and skipped. A title-less create rewrote `parent_id` +
properties over the holder; a minimal same-title re-create re-stamped both
timestamps. The batch seam (`execute_batch_with_origin`: bulk org ingest +
the Loro→SQL projection) ran no gate at all. Reachability corrected after
verification: NOT cold-boot ingest (`QueryableCache::get_by_id` runs a live
`SELECT`, so held rows classify as updates) — the real window is the Loro
incremental projection drifting from its in-memory diff base, which
self-heals via a full walk.

## Root cause

secondary ORACLE, collision-semantics lane (task #8), found by AGENT
ADJUDICATION of the create path — no automated test produced it: **a
`create` whose `id` is ALREADY HELD clobbers the holder whenever the create
carries no title.** `prepare_create` emits an unguarded `INSERT … ON
CONFLICT(id) DO UPDATE SET <every supplied non-id column> = excluded.<col>`,
and the ADR 0029 D1b identity gate in `create_row` was CONTENT-GATED ("with
no title there is nothing to recognize"), so a title-less create skipped
recognition entirely and landed. Measured on the real `block_raw` schema in
`crates/holon/tests/create_id_collision_semantics.rs`: a title-less create
over a held row rewrote `parent_id` and its properties, and even a MINIMAL
same-title re-create (id + content only, the shape a bare `block.create` or
a cache-missed re-observation produces) re-stamped `created_at`/`updated_at`
on a row it changed nothing else about. The batch seam
(`execute_batch_with_origin` — what bulk org ingest and the Loro→SQL
projection write through) ran NO recognition at all. REACHABILITY, corrected
after verification: NOT cold-boot org ingest — `QueryableCache::get_by_id`
delegates to a LIVE `SELECT * FROM block_raw` (`queryable_cache.rs:246-278`,
`:1060`), so a held row is classified an UPDATE on a cold boot and never
reaches the create arm. The genuine window is the LORO INCREMENTAL
projection, whose diff base is the in-memory live snapshot: when that
snapshot drifts from the sink, a held row is emitted as a create. That path
self-heals (`seeded=false` → full walk → update), so the blast radius is one
failed batch plus a surfaced error, not a wedge. FIXED in this lane per the
2026-08-12 ruling: `recognize_create` runs the single-source
`recognize_derived_id` predicate on BOTH seams before anything is minted or
placed — title-less over a held id is a loud refusal, a recognized re-create
keeps its current key and re-asserts only the fields it supplies AND that
differ (nothing at all when it agrees), and the different-title
`IdentityCollision` arm is unchanged.)

## Missing piece

no transition mints a create at an id some OTHER row already holds — the
create alphabet only ever supplies fresh or freshly-minted ids, so the
collision state is ungeneratable; and no invariant says a create must be
identity-preserving at a held id

## Remedy

FIXED — `recognize_create` on both seams (loud refusal when title-less,
diff-guarded re-assert when same-title, D1b unchanged when different); seven
rungs in `crates/holon/tests/create_id_collision_semantics.rs` (red 4/6 →
green 7/7)
