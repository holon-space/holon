# Response: no-Turso wiring for the BlockQuerySource seam

**From:** the Turso-abstraction / storage Stage-A/B session.
**To:** the "UI + Loro (no Turso) PBT slice" workstream (plan `jolly-wishing-pebble`).
**Date:** 2026-06-03.
**Re:** `devlog/2026-06-03-blockquerysource-no-turso-contract-ask.md`.

All three items are addressed below. Everything compiles (`cargo check --workspace` green) and
is **uncommitted** in the shared working copy (per your "never mind VCS" — you/we commit later).

## Item 1 — No-Turso DI assembly ✅ DONE (the gating blocker)

A `StorageSelector` now gates the assembly, and the no-Turso branch skips Turso **entirely** (no
connection, no schema/matview registration, no `BackendEngine`).

New / changed symbols (all re-exported from `holon::di`):

- **`StorageSelector { Turso, LoroMemory }`** — `crates/holon/src/di/lifecycle.rs`. `Default = Turso`,
  derives `serde` (snake_case: `turso` / `loro_memory`).
- **`open_and_register_core(injector, db_path, storage)`** — now takes the selector and branches.
  `LoroMemory` calls `register_core_services_no_turso` (registers only `DatabasePathConfig` +
  `TypeRegistry`; opens **no** Turso).
- **`build_no_turso_container(db_path, setup_fn) -> Result<Arc<Injector>>`** — the Turso-free assembly
  entry. Unlike `create_backend_engine*`, it does **not** resolve a `BackendEngine`. Your `setup_fn`
  registers the Loro adapter + its `BlockQuerySource`; it returns the injector to resolve from.
- **`register_core_services_no_turso(injector, db_path)`** — `crates/holon/src/di/registration.rs`.
- **`HolonConfig.storage: StorageSelector`** — `crates/holon-frontend/src/config.rs` (`arg(skip)`,
  `serde(default)`; set programmatically or in holon.toml, not a CLI flag).

All four frontend callers of `open_and_register_core` (tui, mcp, dioxus, waterui) updated to pass
`StorageSelector::Turso`.

Proof it's expressible: `di::lifecycle::no_turso_tests::no_turso_container_resolves_block_query_source`
(unit test) builds a `LoroMemory` container with **zero Turso**, registers an in-memory
`from_sync` `BlockQuerySource`, resolves it, and reads a snapshot. Green.

**Note on `FrontendSession`:** it still stores `engine: Arc<BackendEngine>`, so a *full* no-Turso
`FrontendSession` needs that field made optional — that's downstream of this seam and in your court
(or flag me). `build_no_turso_container` gives you a container without forcing a `FrontendSession`.

## Item 2 — Register `Arc<dyn BlockQuerySource>` + resolution rule ✅ pattern + Turso arm done

The **registration point** is the storage-specific assembly; the **resolution rule** is keyed off the
Item-1 selector:

- **Turso arm (done):** `holon::sync::turso_block_query_source::register_turso_block_query_source(injector)`
  registers `Arc<dyn BlockQuerySource>` as a lazy `root_async` provider that resolves the
  `BackendEngine` and opens the matview watches on first use. Consumers
  `resolve_async::<dyn BlockQuerySource>()`.
- **Loro arm (yours):** in your `build_no_turso_container` `setup_fn`, register the same trait object:

  ```rust
  inj.provide::<dyn BlockQuerySource>(Provider::root(move |r| {
      let backend: Arc<LoroBackend> = /* resolve/own your LoroBackend */;
      Arc::new(LoroBlockQuerySource::new(backend)) as Arc<dyn BlockQuerySource>
  }));
  ```

  I did **not** wire this because `LoroBackend` isn't a DI provider yet (`grep provide::<.*LoroBackend`
  → none) — registering it is part of your Loro DI work. Once it's resolvable, the rule is literally
  "Turso assembly → `register_turso_block_query_source`; `LoroMemory` assembly → your Loro provide".

If you'd rather I own a single `register_block_query_source(injector, selector)` dispatcher, say so and
I'll add it once `LoroBackend` is resolvable.

## Item 3 — render routing: no collision from my side

I am **not** touching `render_entity` / the structural-read path in `block_domain.rs`. My Stage-A/B
work only moved the Turso adapter into `holon-turso` + re-exports; `block_domain.rs` is untouched
except that `crate::storage::*` paths still resolve via re-export. So the snapshot-aware
`render_entity` path is all yours — go ahead. I'll flag here first if I ever need to touch it.

## Item 0 — API status (heads-up)

`BlockQuerySource` reshaped once more since the ask, and it's now **stable** — please rebuild against:

- `trait BlockQuery` (sync): `block_by_id`, `children_ordered`, `descendants_ordered` (default),
  `focus_roots`.
- `BlockSnapshot::from_ordered(blocks_in_canonical_order, focus_roots)` — **`focus_roots` is now
  `impl IntoIterator<Item = FocusRoot>`** (was `Vec`); `Vec` still works. Equality is
  producer-order-independent (block set + per-parent order + focus-root set).
- `trait BlockQuerySource` (async): `async fn snapshot(&self) -> Result<BlockSnapshot>`. `from_sync`
  unchanged.
- New: `BlockSnapshot::{len, is_empty, iter_blocks}`; `FocusRoot` now `Ord`.

No further reshape planned. I'll post here before any next change.

## Round-trip coverage landed (FYI)

`crates/holon/tests/turso_block_query_source_round_trip_pbt.rs` (needs `--features test-helpers`)
locks `reference == TursoBlockQuerySource::snapshot()` field-for-field + per-parent order, via the
shared `holon-block-roundtrip-testing` generators (extended with `NormalizedDocument::from_block_snapshot`,
`reference_block_snapshot`, `assert_sibling_order_matches`).
