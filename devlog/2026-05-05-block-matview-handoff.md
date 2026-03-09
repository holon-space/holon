# Block matview migration — handoff (2026-05-05)

Worktree: `block-roundtrip-pbt`. Continues from
`devlog/2026-05-04-block-roundtrip-handoff.md` Task #3.

## tl;dr

**Task #3 is mostly landed.** The `block` table is now a matview that
hydrates `tags` + `blocked_by` from junction tables; the underlying
table is `block_raw`; PBT regression gate (`turso_block_round_trip_pbt`,
2 cases) is GREEN.

What's left:
1. **Drop the SUT `live_block_tags` workaround** (~30 lines in
   `crates/holon-integration-tests/src/pbt/sut.rs`) — should be a
   straight delete now that `live_blocks` reads hydrated rows from the
   matview.
2. **Validate broader test suites and a real frontend boot** — only
   the writer-side PBT was run; integration tests + e2e PBTs +
   actually starting a frontend haven't been verified.
3. **Migration story** — dev DBs carrying the old `block` table will
   collide with the new `block` matview at startup. See "Migration
   gotcha" below.
4. **MCP read sites** (Task #4 in the prior handoff) — orthogonal,
   still open. Not blocking.

## What landed in this session

### Refactor along green (paved the path; no behavior change)

| Change | Where |
|---|---|
| `BLOCK_WRITE_TABLE` / `BLOCK_READ_TABLE` consts | `crates/holon/src/storage/block_table_names.rs`, re-exported via `holon::storage::*` and `holon-frontend::BLOCK_READ_TABLE_PUB` |
| Threaded const through 7 writer sites | event_infra_module, holon-orgmode/di, holon-frontend/lib (×3), holon-worker/seed (×2), backend_engine.rs test seed |
| Threaded const through 9 reader sites | SUT live_blocks, MCP tools (×2), holon-worker/lib, holon-frontend/link_provider, dioxus-web, loro_module seed, registration query-source filter, ui_watcher, block_domain |
| Split `CacheBlockReader::load_all_blocks_with_hydration` | `load_all_blocks_via_correlated_subquery()` (now `#[allow(dead_code)]` fallback) + `load_all_blocks_via_matview()` (new live path) |

### The actual flip (this is what changed runtime behavior)

| Change | Where |
|---|---|
| `block` table renamed to `block_raw` | `crates/holon/sql/schema/blocks.sql` (table + index) |
| Junction FKs: `REFERENCES block(id)` → `REFERENCES block_raw(id)` | `task_blockers.sql`, `block_tags.sql` |
| New `block` matview (case2b shape, all 17 columns) | `crates/holon/sql/schema/block_matview.sql` |
| Schema modules split | `CoreSchemaModule` provides `block_raw`. `BlockSchemaModule` provides junction tables (FK to block_raw), no longer creates the task_blocking_edges matview. New `BlockMatviewSchemaModule` provides `block` (the matview). New `TaskBlockingEdgesSchemaModule` owns the chained `task_blocking_edges` matview |
| DI dependency graph | New `BlockMatviewView` + `TaskBlockingEdgesView` markers in `schema_providers.rs`. `BlockHierarchyView` and `NavigationTables` now depend on `BlockMatviewView` (their matviews JOIN `block`, which is now chained) |
| `BLOCK_WRITE_TABLE` = `"block_raw"` | `block_table_names.rs` |
| Reader switch | `CacheBlockReader::load_all_blocks_with_hydration` calls `load_all_blocks_via_matview` (was `_via_correlated_subquery`) |
| PBT fixture aligned | `turso_block_round_trip_pbt.rs::setup_production_schema` runs `CoreSchemaModule` + `BlockSchemaModule` + `BlockMatviewSchemaModule` instead of hand-rolling a `block` table |

### Verification done

- **Chained-matview preflight** (run before any code changes):
  `bigdata/turso/bugs/holon_block_hydration_repro.sql` `CHAIN_*` sections.
  All four shapes holon uses on top of `block` work — simple filter,
  WITH RECURSIVE on matview, matview JOIN base table, two-hop CDC
  (base mutation → block matview → chained matview). One narrow open
  Turso bug (G3) doesn't affect any current holon matview. Memory:
  `turso_chained_matview_supported_2026.md`.
- **PBT regression gate**: `cargo test -p holon --test turso_block_round_trip_pbt`
  → `2 passed; 0 failed`.

## Migration gotcha — production dev DBs

`CoreSchemaModule.ensure_schema()` calls `CREATE TABLE IF NOT EXISTS block_raw (...)`.
A dev DB that already has a `block` *table* (from before this PR) will
**not** have a `block_raw` table, and the `BlockMatviewSchemaModule`
will then try to `CREATE MATERIALIZED VIEW block AS … FROM block_raw …`
— which fails because `block_raw` doesn't exist and `block` already
does (as a table, not a matview).

Two options for the migration:
- **(a) clean slate**: tell users to nuke their dev DB on first run
  past this PR. Lowest engineering cost; acceptable for pre-1.0.
- **(b) data migration**: add a one-shot startup hook that detects an
  existing `block` table, renames it to `block_raw`, drops any leftover
  matview state, then proceeds. About 20 lines in
  `CoreSchemaModule::ensure_schema` or a dedicated migration module.

I'd default to (a) and ship a CLAUDE.md note. We're far from any
real-user data.

## Phase D — drop SUT `live_block_tags`

Tomorrow's first job. `crates/holon-integration-tests/src/pbt/sut.rs`:

- ~line 97-117: `BlockTag` struct + its doc comment about why this
  workaround exists. **Delete.**
- ~line 3022-3032 (the doc comment on `live_blocks`): rewrite. The
  matview now hydrates tags/blocked_by, so the comment about "Tags are
  *not* included here…" is no longer true.
- `live_block_tags()` accessor + `live_blocks_tags_cell` field on the
  SUT. **Delete.**
- `check_invariants_async`: remove the manual fold from `live_block_tags`
  into `block.tags`. The block rows from `live_blocks` already carry
  hydrated `tags`.

The `parse_block_row` helper in sut.rs already accepts `tags` from the
row (it tolerates the column being absent). The matview's row shape
includes `tags` as a JSON string — `Block::try_from(HashMap)` parses it
correctly via the hand-rolled deserializer (see memory
`block_two_deserializers.md`). So the SUT cleanup is genuinely just
deletion.

## Open questions / risks for tomorrow

1. **Other tests beyond the PBT** — only the 2-case PBT was run. Worth a
   `cargo nextest run -p holon` and a focused run of
   `general_e2e_pbt`. Likely failures: any test that hand-rolls
   `CREATE TABLE block` instead of going through schema modules. Search
   pattern: `rg -nP 'CREATE TABLE.*\bblock\b' --type rust`. Already
   noted: 4 repro files in `crates/holon/src/storage/*_repro.rs` —
   self-contained, fine to leave; they don't exercise the production
   schema modules.

2. **Frontend boot** — actually start the GPUI frontend (`cargo run -p holon-gpui`)
   and verify the boot path doesn't trip on the rename. The
   `BlockHierarchyView` and `NavigationTables` providers now wait on
   `BlockMatviewView` — if FluxDI's eager resolver doesn't pick the new
   marker up automatically, boot would hang. (Should be fine — both new
   markers are listed in `all_schema_roots()`, but worth confirming.)

3. **`task_blocking_edges` chained shape** — the chained-matview
   preflight passed, but the holon-side test that exercises this
   matview at runtime is the `task_blocking_edges` integration test
   (if one exists) or a Petri-net test. Worth a smoke test.

4. **CDC propagation through the new chain** — verify by running a PBT
   that mutates blocks and checks `block_with_path` reflects the change.
   The preflight verified two-hop CDC at the SQL level; the holon
   matview-CDC path involves `MatviewManager::watch` and `LiveData` on
   top, which has its own subscription dance. Worth a focused
   `cargo nextest run -p holon-orgmode --features di --test round_trip_pbt`
   and `general_e2e_pbt` to confirm.

## Where the boundaries are

- **Things that should _not_ change** when reverting just the const:
  the SUT live_block_tags workaround, the schema module split,
  the new matview SQL file, the PBT fixture. Those are durable
  improvements regardless of the matview flip.
- **Things tied to the const value**: `CacheBlockReader` reader-path
  selection, `BLOCK_WRITE_TABLE` literal, the schema modules creating
  `block_raw` instead of `block`. Setting `BLOCK_WRITE_TABLE = "block"`
  + flipping the reader-path call back + reverting the SQL files would
  put us back at the pre-Task-#3 world.

## Re-running the gate

```sh
cargo test -p holon --test turso_block_round_trip_pbt
# expect: 2 passed; 0 failed
```

## Cross-repo state

Turso side: `bigdata/turso` branch `holon` at
`b454325b038f35291e12aa763fe3f6f5de703b5a` (= `2f61ca4eee2c97b485462658cd005db45244da43`
from holon's Cargo.lock perspective; cosmetic doc/test diffs since
then). G0 + G1 fixed; G2 open and non-blocking; G3 (aliased base case
+ `p.id` reference in WITH RECURSIVE on a matview) noted in
`bigdata/turso/bugs/holon_block_hydration_matview_gaps_2026-05-04.md`
and the `turso_chained_matview_supported_2026.md` memory.

## Files touched (this session, after the preflight work)

```
crates/holon/sql/schema/blocks.sql
crates/holon/sql/schema/block_matview.sql                       (new)
crates/holon/sql/schema/block_tags.sql
crates/holon/sql/schema/task_blockers.sql
crates/holon/src/storage/block_table_names.rs                   (new)
crates/holon/src/storage/mod.rs
crates/holon/src/storage/schema_modules.rs
crates/holon/src/di/schema_providers.rs
crates/holon/src/sync/event_infra_module.rs
crates/holon/src/sync/loro_module.rs
crates/holon/src/api/backend_engine.rs                          (test fixture only)
crates/holon/src/api/block_domain.rs
crates/holon/src/api/ui_watcher.rs
crates/holon/src/di/registration.rs
crates/holon/tests/turso_block_round_trip_pbt.rs
crates/holon-orgmode/src/di.rs
crates/holon-frontend/src/lib.rs
crates/holon-frontend/src/link_provider.rs
crates/holon-integration-tests/src/pbt/sut.rs                   (live_blocks SQL only — workaround still present)
frontends/mcp/src/tools.rs
frontends/holon-worker/src/lib.rs
frontends/holon-worker/src/seed.rs
frontends/dioxus-web/src/main.rs
bigdata/turso/bugs/holon_block_hydration_repro.sql              (preflight)
bigdata/turso/bugs/holon_block_hydration_matview_gaps_2026-05-04.md
```

No commits yet — the worktree is still uncommitted; pick up from
`jj status` tomorrow.
