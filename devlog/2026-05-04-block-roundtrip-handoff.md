# Block round-trip PBT — handoff

Worktree: `block-roundtrip-pbt`. Session 2026-05-04.

This worktree closed six tasks (#1, #2, #5, #6, #7) and surfaces two
follow-ups (#3 and #4) that are out of scope for this session.

## What's in the worktree

| Layer | Change |
|---|---|
| New shared crate | `crates/holon-block-roundtrip-testing/` — generators + `assert_normalized_docs_equal` |
| Org PBT | `crates/holon-orgmode/tests/org_block_round_trip_pbt.rs` — round-trips through `OrgFormatAdapter` |
| Turso PBT | `crates/holon/tests/turso_block_round_trip_pbt.rs` — two cases: writer-side junction fan-out + full Block round-trip via `CacheBlockReader` |
| Existing PBT | `crates/holon-orgmode/tests/round_trip_pbt.rs` lifted (-518 net, imports the shared crate) |
| Production fix | `CacheBlockReader::get_blocks` now hydrates `tags` from `block_tags` via correlated subquery (`crates/holon-orgmode/src/di.rs`) |
| Production fix | `SqlOperationProvider::partition_params` no longer drops every `_`-prefix key — only `_routing_*` and `_expected_*` (`crates/holon/src/core/sql_operation_provider.rs`) |
| Production fix | `block_to_params` no longer writes a redundant no-underscore copy of `source_header_args` (`crates/holon/src/sync/loro_sync_controller.rs`) |
| API | `block_to_params` promoted from `pub(crate)` to `pub` and re-exported from `holon::sync` |
| API | `QueryableCache::db_handle()` accessor added (`crates/holon/src/core/queryable_cache.rs`) |
| Test infra | SUT doc comments at `crates/holon-integration-tests/src/pbt/sut.rs:97-117` and `:3022-3032` updated to explain that `live_block_tags` is **not** a workaround for a prod bug — it's required by Turso IVM matview limitations |

Build: clean. Both new Turso PBT cases: green 30/30 each (~1.0s). Org PBTs:
same pre-existing flakes as on plain main (verified by re-running on the
non-worktree checkout).

## Pinned memory entries (this session)

- `feedback_storage_abstraction_no_leak.md` — junction data must be hydrated
  by the read path; renderer/writer/etc. should never query junctions directly.
- `turso_ivm_no_array_aggregation_in_matviews.md` — `CREATE MATERIALIZED VIEW`
  rejects `json_group_array` and `group_concat`; one-shot SELECTs work.
- `block_two_deserializers.md` — for SQL rows, use `Block::try_from(HashMap)`;
  the derived `<Block as TryFromEntity>::from_entity` silently returns
  `tags = []` because the field is `#[serde(skip, default)]`.

---

## Task #3 — Promote `block` to a matview that hydrates junctions (BLOCKED)

**Status:** parked, blocked on upstream Turso IVM. Re-open as soon as the
holon Turso fork (`bigdata/turso`, branch `holon`) ships `json_group_array`
or `group_concat` in matview definitions.

**Goal:** rename the current `block` table to `block_raw`, then make `block`
a matview that LEFT JOINs `block_raw` with `block_tags` (→ `tags` JSON array)
and `task_blockers` (→ `blocked_by` JSON array). Every consumer reads the
hydrated `block` rows and never has to know about junction tables.

**Validated spike (May 4) via `holon-direct`:**

```sql
-- works (regular VIEW with correlated subquery)
CREATE VIEW spike_block_hydrated AS
SELECT b.*,
  (SELECT json_group_array(tag) FROM block_tags WHERE block_id = b.id) AS tags,
  (SELECT json_group_array(blocker_id) FROM task_blockers WHERE blocked_id = b.id) AS blocked_by
FROM block b;

-- fails (the same shape inside CREATE MATERIALIZED VIEW)
CREATE MATERIALIZED VIEW … json_group_array(…)  -- rejected at DDL
CREATE MATERIALIZED VIEW … group_concat(…)      -- rejected at DDL

-- works (scalar aggregations)
CREATE MATERIALIZED VIEW … COUNT(*), MIN(tag), MAX(tag)  -- accepted
```

So the architecture is sound; the missing primitive is array/string
aggregation in the IVM operator graph. File a Turso ticket if not already
filed — the use case (hydrating edge-typed fields back into entity views) is
the obvious motivator.

**Once Turso IVM supports `json_group_array`:**

1. Rename table: `block` → `block_raw`. Update every `FROM block` /
   `INSERT INTO block` site outside the matview definition. Audit:
   `rg "FROM block\\b|INTO block\\b"`.
2. Create `block` matview with the spike shape above.
3. Switch `CacheBlockReader::load_all_blocks_with_hydration` (currently does
   the correlated subquery itself in `crates/holon-orgmode/src/di.rs`) to
   plain `SELECT * FROM block`. Same `Block::try_from(row)` deserializer.
4. Switch the SUT's `live_blocks` SELECT (`crates/holon-integration-tests/src/pbt/sut.rs:2996`)
   to also read from the new `block` matview. Drop `live_block_tags`,
   `BlockTag`, and the manual fold in `check_invariants_async` (~30 lines).
5. Run all PBTs. The Turso PBT (`turso_block_round_trip_pbt.rs`) is the
   regression gate: stays green if (3) is correct.

**Why this matters beyond cleanup:** today the SUT and prod use *different*
read paths for the same conceptual operation (hydrated block read). One
matview-backed `block` view collapses both into the same code path, removes
the SUT's `live_block_tags` workaround, and makes "consumers must not query
junctions" a structural invariant rather than a documented convention.

**Related memory:** `turso_ivm_no_array_aggregation_in_matviews.md`,
`turso_cdc_only_via_matviews.md`, `turso-chained-matview-hang` skill (verify
the new chained matview — Page filter on top of `block` matview — doesn't
hit the existing chained-matview hang).

---

## Task #4 — Migrate MCP render-org tools to read via `BlockReader`

**Status:** open, orthogonal — can land any time after #5 (✅ done).

**Goal:** `frontends/mcp/src/tools.rs` currently issues raw SQL for block
reads, then renders org via `OrgRenderer::render_entitys` directly. Today
it re-introduces the same hydration leak that #5 closed for the org-sync
controller. Migrate the MCP tools to read via `BlockReader::get_blocks`
(or similar) so MCP-rendered org gets the same hydrated-tags guarantee.

**Specific call sites (`frontends/mcp/src/tools.rs`):**

- `tools.rs:1005` — `"SELECT * FROM block WHERE id = $1"` → replace with
  a `BlockReader`-style by-id lookup. (BlockReader doesn't have a
  `get_by_id` today; either add one or use `iter_documents_with_blocks`
  / scan + filter.)
- `tools.rs:1207` — `"SELECT * FROM block WHERE parent_id LIKE $doc_uri_prefix"`
  → switch to `BlockReader::get_blocks(doc_id)` (already returns descendants
  of the doc with hydrated tags).
- `tools.rs:1355` — calls `OrgRenderer::render_entitys(&blocks, …)` with
  the result of those raw SQL reads. Stays unchanged once the input blocks
  are hydrated.

**How to wire `BlockReader` into the MCP tool harness:**

The MCP server already has access to the `BackendEngine`. Get a
`CacheBlockReader` either by:
- (a) Calling out to `holon_orgmode::di::CacheBlockReader::new(cache)` with
  a `QueryableCache<Block>` resolved from the same DI container the org-sync
  controller uses. Same pattern as `crates/holon-orgmode/src/di.rs:713-714`.
- (b) Constructing it ad-hoc from the engine's `db_handle()` (the same path
  the new `turso_block_round_trip_pbt.rs` uses for testing).

(a) is cleaner since it shares the lifecycle with prod. (b) is a one-liner
if DI access is awkward from the MCP frontend.

**Regression gate:** there isn't a dedicated MCP-side PBT today, but the
behavior is now testable through the shared crate. If you want one, the
shape mirrors `org_block_round_trip_pbt.rs` — generate blocks, write via
the tool's create endpoint, render via the tool's render endpoint, compare.
Smaller scope is fine: a single test asserting that an MCP-rendered doc
preserves headline tags after a round-trip.

**Validation that this is real:** the same `:edge_abstraction:` headline
tag drop bug we hit on May 4 lives latent in MCP's render path today.
Anyone using the MCP `render_org_from_blocks` tool will silently lose
headline tags. That's why this task exists separately from #5.

---

## Dependency graph for the remaining work

```
   #3 (block-as-matview)              #4 (MCP → BlockReader)
        ↑                                   ↑
        |                                   |
        +— blocked on Turso IVM             +— blocked on nothing
           (your bigdata/turso work)           (orthogonal cleanup)

   #1, #2, #5, #6, #7 — ✅ all done in this worktree
```

Land order: **#4 anytime**, **#3 once Turso IVM ships** `json_group_array`
in matviews. After #3, the SUT's `live_block_tags` collapses naturally
(also closes the architectural-leak class for good).

## Re-running the PBTs

```sh
# Turso side (the one that proves #5/#6/#7 are correct)
cargo nextest run -p holon --test turso_block_round_trip_pbt

# Org side — surfaces a separate pre-existing source-block escape bug,
# which the worktree didn't touch
cargo nextest run -p holon-orgmode --features di --test org_block_round_trip_pbt

# Existing round-trip — pre-existing flake on master too
cargo nextest run -p holon-orgmode --features di --test round_trip_pbt
```

Build everything: `cargo build --tests --workspace --features di`.
