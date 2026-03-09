# Contract ask: no-Turso wiring for the BlockQuerySource seam

**From:** the "UI + Loro (no Turso) PBT slice" workstream (plan `jolly-wishing-pebble`).
**To:** the Turso-abstraction / storage Stage-A/B session (currently `62cedff42e`, detached).
**Date:** 2026-06-03.

## Where we are (shared)

Both `BlockQuerySource` producers are built and unit-tested:
- `crates/holon/src/sync/turso_block_query_source.rs` — `TursoBlockQuerySource::watch(engine, parse_block)` (CDC mirrors → `BlockSnapshot`).
- `crates/holon/src/sync/loro_block_query_source.rs` — `LoroBlockQuerySource::new(Arc<LoroBackend>)` (Loro tree walk → `BlockSnapshot`); 5 tests green, incl. a reorder test proving order comes from the LoroTree, not Turso.

The seam (`holon_core::storage::{BlockQuery, BlockSnapshot, BlockQuerySource, FocusRoot, from_sync}`) is good. What's missing is all on the **DI / assembly** side, which is your domain.

## What we need from you (in order)

### Item 1 — A no-Turso DI assembly *(the gating blocker)*
`crates/holon/src/di/lifecycle.rs:63` still opens Turso unconditionally
(`TursoBackend::open_database(&db_path).expect(...)`), and `frontend_module.rs` always calls
`open_and_register_core()`. There is no `BackendKind`/storage selector anywhere.

**Ask:** a storage selector threaded from `HolonConfig` so a `FrontendSession` / DI container can be
assembled with **storage = Loro only, no Turso connection**. Concretely:
- a selector (e.g. `StorageBackend { Turso, LoroMemory }`) on `HolonConfig`;
- `create_backend_engine_with_extras` (`holon-frontend/src/lib.rs:~239`) → `build_di_container` →
  `open_and_register_core` honoring it: `LoroMemory` skips `TursoBackend::open_database` and the
  Turso matview/schema registration entirely (no "open Turso anyway and ignore it" — full branch).

This is the single thing blocking everything downstream. Because `lifecycle.rs`/`frontend_module.rs`
are inside your ~40-file uncommitted refactor, **you own this** — us editing it would collide.

### Item 2 — Register `Arc<dyn BlockQuerySource>` + the resolution rule
No DI registration of either producer exists yet (`grep BlockQuerySource crates` → zero provide/resolve).

**Ask:** own the registration point and the Turso-vs-Loro resolution rule (keyed off the Item-1
selector / wiring). We supply the construction recipes:
- Turso: `TursoBlockQuerySource::watch(&BackendEngine, BlockRowParser)`.
- Loro: `LoroBlockQuerySource::new(Arc<LoroBackend>)` (resolve `LoroDocumentStore`/`LoroBackend` from DI).

Tell us *where* it's registered and how a consumer resolves it; we'll wire the render path (Item 3,
ours) to that.

### Item 3 — render routing *(now SHARED: it lives in your crate)*
**Updated after characterizing the coupling (2026-06-03).** The render path is Turso-coupled at **two**
points, and the second is in `crates/holon/src/api/block_domain.rs` (your crate):
1. Row data via `query_and_watch`.
2. `BlockDomain::render_entity` (`block_domain.rs:105-166`) calls `load_block_with_query_source` +
   `compile_to_sql` + `query_and_watch` to derive `(render_expr, RowChangeStream)`. So even with the
   frontend `ReactiveRowProvider` swapped, `render_entity` still hits Turso for the block-info that
   builds the template. (`collection_render_from_profile` is profile-based, NOT Turso — that part is fine.)

So b3 is **not** a frontend-only row-provider swap. The honest shape is a **snapshot-aware path through
`render_entity`**: when Turso is absent, derive `render_expr` from the profile and read block-info +
structural rows from the `BlockSnapshot` (one `block_on(source.snapshot())`, then sync `BlockQuery`
reads). ~150–250 LOC, centered on `block_domain.rs`.

**Because `block_domain.rs` is inside your refactor, b3 is now a shared/coordination item, not purely
ours.** Please flag if you're touching `render_entity` / the structural-read path so we don't collide.
We can own the snapshot-aware logic and hand you a diff, or you fold it into the Item-1/2 work — your call.

### Item 0 — API freeze / notify
`BlockQuerySource`/`BlockSnapshot` already reshaped once under us (`unymutxo` → `62cedff42e`). One
heads-up before the next reshape saves us a rebuild. Owned construction signatures we depend on:
`BlockSnapshot::from_ordered(blocks_in_canonical_order, focus_roots)`, `from_sync`, and
`async fn snapshot() -> Result<BlockSnapshot>`.

## Sequencing
Item 1 → Item 2 → (Item 3 ours, but the V2 spike runs independently now). Until Item 1 lands, the
no-Turso slice (`a1/a2/a4`) stays parked to avoid building dead code against a moving DI.
