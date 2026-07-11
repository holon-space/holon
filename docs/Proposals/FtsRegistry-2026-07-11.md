# FTS + Engine-Function Registry — 2026-07-11

Status: implemented (C3 stream). Rulings (Martin, binding): FTS = the Turso
fork's Tantivy index method; registry = generalize the fork's Func-enum
resolution path, split **by function shape** — scalar/predicate via the Func
enum, set-valued (relation-returning) via the matview/TVF declaration path.

## 1. Feature plumbing

The fork already ships FTS: `core/index_method/fts.rs` (Tantivy behind a
BTree-backed `Directory`), SQL surface `CREATE INDEX .. ON t USING fts (cols)
[WITH (tokenizer=.., weights=..)]`, functions `fts_match` / `fts_score` /
`fts_highlight` resolved in `core/function.rs`, feature `fts = ["dep:tantivy"]`.

Holon enables it in the root workspace `Cargo.toml`:

```toml
turso      = { .., default-features = false, features = ["fts"] }
turso_core = { .., default-features = false, features = ["json", "fts"] }
```

(`turso/fts → turso_sdk_kit/fts → turso_core/fts`; `turso_core` listed
explicitly because several crates depend on it directly.)

**Second gate — experimental index method (runtime, not just build).** The
`fts` cargo feature only *compiles in* the code. `CREATE INDEX .. USING fts`
additionally requires the connection's experimental-index-method flag, else it
fails loud: `Parse error: index method is an experimental feature. Enable with
--experimental-index-method flag`. Holon enables it in
`crates/holon-turso/src/turso.rs::TursoBackend::open_database` (the `cfg(unix)`
native path) via `DatabaseOpts::default().with_views(true).with_index_method(true)`.
This is the same flag that gates the sparse-vector method, so enabling it now
also unblocks the future `similar()`/embeddings work. The wasm `open_database`
leaves it off (fts is `cfg`'d out of `turso_core` on wasm regardless).

**wasm handling** — fts stays OFF on wasm through two independent mechanisms:

1. The wasm frontends (`frontends/holon-worker`, `frontends/dioxus-web`) are
   `exclude`d from the root workspace and carry their own `Cargo.toml` +
   lockfiles + turso git deps (holon-worker even pins a different branch,
   `holon-wasm-fix2`). Workspace feature unification cannot reach them; nothing
   was changed there.
2. Defense in depth: every fts item in `turso_core` is gated
   `#[cfg(all(feature = "fts", not(target_family = "wasm")))]` (same pattern as
   `load_extension`), so even a wasm build that requested the feature compiles
   it out.

A dedicated wasm target check is not cheaply runnable from the root workspace
(`cargo check --workspace` explicitly excludes the wasm crates; they build via
`napi build` / `trunk build`). The feature-graph reasoning above is exact:
features flow only through workspace dependency edges, and there is no edge
from this workspace to the wasm crates' turso deps.

## 2. Maintenance contract — verdict: WRITE-MAINTAINED (empirically confirmed)

The index method trait (`core/index_method/mod.rs`) has `insert`/`delete`
hooks driven by the VDBE on DML; updates are delete+insert (Tantivy tombstone
model, merged at segment merge).

Verified empirically holon-side (not just fork-side) in
`crates/holon/tests/fts_e2e.rs` through the real `DatabaseActor`/`DbHandle`
stack:

- INSERT after index creation → immediately visible to `fts_match`.
- UPDATE → new terms match, stale terms stop matching.
- DELETE → row stops matching.
- `fts_score` orders results over the maintained index.

No rebuild step exists or is needed. (Fork-side corroboration:
`tests/integration/index_method/mod.rs::test_fts_comprehensive_lifecycle`,
`test_fts_with_explicit_transactions`.)

Operational notes: Tantivy writer batches commits (`BATCH_COMMIT_SIZE = 1000`)
inside the fork's index method; `optimize` entry points exist
(`test_fts_optimize_index`) for segment merging — not needed for correctness.

## 3. What works end-to-end today

With the feature on, a `holon_sql` source block is pass-through
(`BackendEngine::compile_to_sql` applies transformers but no function
whitelist — scouted `crates/holon/src/api/backend_engine.rs:269-276`), so:

```sql
CREATE INDEX fts_block_content ON block_raw USING fts (content);
SELECT id, content FROM block_raw WHERE fts_match(content, 'query terms');
```

Covered by `crates/holon-integration-tests/tests/fts_query_block_e2e.rs`
(both **PASS**):

- `fts_direct_query_over_block_content` — one-shot query over block content.
- `fts_query_block_live_path` — the full query-block route
  (`compile_to_sql` → `query_and_watch`), which materializes the query as a
  matview.

**KEY VERDICT — `fts_match` works inside a materialized view.** The live path
(`query_and_watch` → `MatviewManager::ensure_view` → `subscribe_cdc` →
`query_view`) materialized a matview whose `WHERE` clause is
`fts_match(content, 'tantivy')` and returned exactly the matching block ids
(`block:f1`, `block:f3`). So Turso IVM/DBSP accepts the fts function in a
matview logical plan — fts query blocks are LIVE, not just one-shot. No
fork-side planner change was needed.

The FTS index lives on the base table (`block_raw`), not on matviews — index
methods attach to tables. Queries against the `blocks` matview surface would
need either an index on the matview's backing store (fork-side work) or
predicate pushdown to the base table; v1 queries target `block_raw`.

## 4. Registry shape (holon side)

`crates/holon-turso/src/engine_functions.rs` — insert-only declaration point:

```rust
EngineFunctionDecl { name, arity: Exact(n) | AtLeast(n),
                     shape: Scalar | Predicate | SetValued,
                     dual_evaluable: bool }
ENGINE_FUNCTIONS: &[EngineFunctionDecl]   // fts_match, fts_score, fts_highlight
engine_function(name) -> Option<&'static EngineFunctionDecl>
```

- **Scalar / Predicate** (the `fts_match`/`fts_score` class): resolution
  already happens engine-side via the fork's `Func` enum
  (`core/function.rs:~1791`). The holon registry documents and passes through;
  it is the source of truth for tooling (validation, completion, guard
  classification), NOT a second resolver.
- **SetValued** (future `similar(block, k)`): declared with
  `FunctionShape::SetValued`, resolved via the matview/TVF path — the function
  names a maintained relation, not a row-at-a-time callable. Implementation is
  a documented stub until the first such function ships; the wiring target is
  `matview_manager` (`ensure_view`/`register_fdw_table`).

## 5. How similar()/embeddings slot in later

The fork's index-method seam is the extension point: the same
`IndexMethod`/`IndexMethodAttachment` trait that hosts fts also hosts the
sparse-vector index method (`vector32_sparse` already appears in the fork's
index_method tests). The path:

1. Fork side: dense/sparse embedding index method (`USING vector` /
   `USING sparse_vector`), write-maintained like fts.
2. Scalar rung first: `vector_score(col, query_vec)` — declare as `Scalar`,
   Func-enum resolution, usable in `ORDER BY` exactly like `fts_score`.
3. Set-valued rung: `similar(block, k)` = top-k relation → `SetValued`
   declaration; materialized as a maintained relation (matview/TVF), joined by
   the query planner. Embedding COMPUTATION (content → vector) stays outside
   the engine (effect-producing, ADR 0024 lease/effect-id territory); the
   engine only indexes and searches stored vectors.

## 6. Pattern-guard usage (ADR 0024)

ADR 0024 dual-evaluates Pattern guards (SQL + in-memory). **`fts_match`
CANNOT be dual-evaluated in memory**: it consults the Tantivy index inside the
database file; there is no in-memory implementation, and reimplementing
tokenizer+BM25 semantics holon-side would fork correctness. Consequently:

- The registry carries `dual_evaluable: false` for the whole fts_* family.
- The guard planner must classify guards containing any
  `dual_evaluable: false` function as **SQL-only**: evaluated by watching the
  materialized guard query (the same matview path the weaver already uses),
  never by the in-memory evaluator.
- `fts_highlight` is nominally standalone (works without an index), but it is
  classified SQL-only too — one rule for the family, no special cases.

## 7. Follow-ups

- [x] Live path (`query_and_watch` matview over `fts_match`) — CONFIRMED
      working (§3); no fork-side planner change needed. Remaining: exercise
      incremental CDC maintenance of an fts matview (insert a block after the
      view exists and assert the stream emits it) — the direct-table test
      proves index maintenance; the matview-CDC delta path is untested.
- [ ] Wire `engine_functions::engine_function` into holon_sql validation once
      a validation boundary exists (today SQL is verbatim pass-through; errors
      surface loud from the engine, which satisfies fail-loud).
- [ ] Guard planner: consume `dual_evaluable` when ADR 0024 Phase 2 lands.
- [ ] FTS over the `blocks` matview surface (vs `block_raw`) — needs fork-side
      index-on-matview or pushdown; parked until a real query needs it.
- [ ] Tokenizer/weights defaults for block content (e.g. `ngram` for
      autocomplete) — product decision, not wiring.
