---
date: "2026-04-14"
title: "Holon wasm browser demo — Phase 2 progress"
project: "holon"
---

## Where we are

Phase 2 of the browser demo (devlog/handoff-2026-04-14-holon-wasm-browser-demo.md) is partially landed:

- `frontends/dioxus-web/` exists as a workspace member. `cargo check --target wasm32-unknown-unknown -p holon-dioxus-web` is green.
- `dx build --platform web --release` produces a working wasm bundle (~87 MB unstripped — wasm-opt crashes on the DWARF emitter, see Phase 3 todo).
- The wasm boots in Chromium under Playwright. Turso `:memory:` opens, DDL runs, `BackendEngine` resolves, the schema migration completes (seen `DDL completed successfully` and `Marked 1 resources as available: ["watch_view_d77ac41ba85c1706"]`).
- Dioxus mounts the React-style `App` component and shows the "Booting in-memory backend…" placeholder while the backend bootstrap runs.

**Update (2026-04-14, second session)**: the unreachable is fixed. Root cause: `TursoBackend::execute_ddl_with_deps` and `execute_ddl_auto` use `tokio::time::timeout(...)` to detect missing `mark_available()` calls. Constructing the `Sleep` future calls `tokio::runtime::scheduler::Handle::current()`, which has no runtime under `wasm_bindgen_futures::spawn_local` and compiles to a wasm `unreachable` instruction (panic = abort eats the message). Both call sites now `#[cfg]` away the timeout on wasm32 and `await response_rx` directly. Two boot-path `tokio::spawn` sites in `live_data.rs::subscribe` and `entity_profile.rs::ProfileResolver::with_type_profiles` were also migrated to `crate::util::spawn_actor` while debugging.

After the fix the dioxus-web release bundle boots cleanly in Chromium: `Backend ready.` renders, console shows 0 errors / 0 warnings.

**Stretch goal landed**: dioxus-web `App` now seeds 1 doc + 3 child blocks via `engine.db_handle().execute(...)` with `turso::Value` params after boot, then runs `SELECT id, content FROM block WHERE parent_id = 'doc:hello' ORDER BY sort_key` and renders the rows as a `<ul>`. Verified end-to-end in Playwright: three list items show real `block:hello-child-{0,1,2}` IDs and content sourced from the in-memory Turso `:memory:` DB, console clean.

**Stretch goal #4 landed**: an "Add a block" button in the dioxus-web `App` calls `engine.db_handle().execute(INSERT ...)` from an `onclick` handler via `wasm_bindgen_futures::spawn_local`, then re-runs `query_children` and updates the boot signal. Verified live in Chromium: clicking once produces a 4th list item with a fresh `block:hello-extra-{millis}` id; clicking again produces a 5th. Console stays clean across both clicks. This proves writes + re-render work end-to-end in the browser, even though we're not yet on the reactive `watch_ui` CDC path.

**Reactive CDC landed (third session, same day)**: dioxus-web `App` now uses `engine.query_and_watch(SELECT ... WHERE parent_id = 'doc:hello', ...)` to subscribe to a live `RowChangeStream` and reconciles `Change::{Created,Updated,Deleted,FieldsChanged}` into a `BTreeMap<String, DemoBlock>` inside `consume_cdc`. The "Add a block" handler now ONLY runs an INSERT — the new row arrives via CDC and re-renders automatically. UI shows a running `events_received` counter alongside the block count; clicking the button increments it from `3 → 4 → 5`, proving the matview pipeline + `prepend_initial_data` + demux are all alive.

To unblock this, the following `tokio::spawn` sites were migrated to runtime-agnostic spawn helpers:

- `crates/holon/src/api/ui_watcher.rs` (7 sites): `run_reactive_watcher`, the four `merge_triggers` forwarders (Initial, structural, command, profile-version), `render_and_forward`, and `enrich_stream`. All now use `crate::util::spawn_actor`.
- `crates/holon/src/api/block_domain.rs::render_leaf_block` (1 site): the leaf-block batch emitter, also `spawn_actor`.
- `crates/holon/src/api/backend_engine.rs::prepend_initial_data` (1 site): the channel that emits the first `Created` batch ahead of CDC. This is what allows `query_and_watch` to deliver initial data without a tokio runtime.
- `crates/holon-api/src/reactive.rs` (5 sites): `scan_state_spawn`, `switch_map_spawn`, `combine_latest`, `coalesce`, plus `switch_map`'s inner forwarder. A local `spawn_actor` helper was added (`tokio::spawn` on native, `wasm_bindgen_futures::spawn_local` on wasm). `switch_map` now uses `futures::future::{AbortHandle, Abortable}` for the previous-inner-stream cancellation, since wasm `spawn_local` returns no `JoinHandle` to call `.abort()` on. `holon-api` gains direct `wasm-bindgen-futures` and `futures` deps.

Native `cargo check -p holon -p holon-api` still green.

## What got fixed to get this far

### 1. Tokio reactor on wasm

Holon's `TursoBackend` actor and `MatviewManager::spawn_demux` use `tokio::spawn`, which panics with "no reactor running" on wasm32 because tokio's wasm runtime is never actually polled (Dioxus drives futures via `wasm_bindgen_futures` through the browser microtask queue).

Added `crate::util::spawn_actor` that maps to `tokio::spawn` on native and `wasm_bindgen_futures::spawn_local` on wasm. Migrated the actor + demux + row_changes spawn sites. `wasm-bindgen-futures = "0.4"` added to holon's wasm32 deps.

There are ~50 other `tokio::spawn` sites in the holon crate that we have NOT migrated. They aren't on the boot path so they don't fire during init, but any user interaction that triggers them will panic. Future cleanup: run a Python script over the codebase to swap them all (most are fire-and-forget actors).

### 2. std::time on wasm

`std::time::Instant::now()` and `std::time::SystemTime::now()` panic with "time not implemented on this platform" on wasm32. The `nightscape/turso@cdd46b1c` upstream commit fixes turso_core's `clock.rs` to use `web-time` (good — verified after `cargo update -p turso_core`), but holon's own code had its own callsites:

- `holon/src/di/lifecycle.rs:157` — `Instant::now()` for bootstrap timing
- `holon/src/core/sql_operation_provider.rs:297, 777` — `SystemTime::now()` for `created_at`/`updated_at`
- `holon/src/api/holon_service.rs:114, 132` — query timing
- `holon/src/api/memory_backend.rs:122` — `now_millis()`
- `holon/src/api/loro_backend.rs:421` — `now_millis()`

Added `crate::util::now_unix_millis()` and `crate::util::MonotonicInstant` that route through `web-time` on wasm. `web-time = "1"` added to holon's wasm32 deps. All five callsites migrated.

NOTE: turso_core itself still has `std::time::SystemTime::now()` in several files (`functions/datetime.rs`, `translate/plan.rs`, `vdbe/sorter.rs`, `vdbe/rowset.rs`, `storage/btree.rs`, `storage/page_cache.rs`, `storage/slot_bitmap.rs`). Most are inside `#[test]` blocks and not compiled in release wasm, but `functions/datetime.rs:199 set_to_current` and `translate/plan.rs:2898/2931/3155 ChaCha8Rng::seed_from_u64` are in production code. They didn't fire during the matview creation we observed but will fire when SQL evaluates `datetime('now')` or when the planner picks a hash seed. Upstream fix needed (extend the cdd46b1c pattern to those sites).

### 3. fluxdi rt-multi-thread

Already covered in the previous commit. The local `/Users/martin/Workspaces/rust/fluxdi` fork drops `rt-multi-thread` from `tokio` features. Holon's root Cargo.toml uses `path = "/Users/martin/Workspaces/rust/fluxdi/fluxdi"`.

### 4. Turso git checkout workspace-hack stub

`~/.cargo/git/checkouts/turso-.../c4dc3fc/workspace-hack/Cargo.toml` and `~/.cargo/git/checkouts/turso-.../cdd46b1/workspace-hack/Cargo.toml` are both edited in-place to stub out the hakari-managed dependencies (which would unify tokio full, mio, hyper-server, pyo3, rusqlite-bundled, none of which build on wasm32). These edits will be erased by `cargo clean` or any upstream re-fetch — long-term fix is upstream-side hakari refactor or a turso branch that gates workspace-hack to non-wasm targets.

### 5. Dioxus 0.7 features

`frontends/dioxus-web/Cargo.toml` lists `dioxus = { default-features = false, features = ["web", "macro", "html", "hooks", "signals", "launch"] }`. Without `launch` the `dioxus::launch` function isn't generated; without `macro` the `#[component]` and `rsx!` macros aren't visible.

## Remaining errors before Phase 2 is done

The console shows `RuntimeError: unreachable` immediately after `Marked 1 resources as available: ["watch_view_d77ac41ba85c1706"]`. This is in the matview wiring path. Strategies for the next session:

1. Build with `RUSTFLAGS="-C debuginfo=2"` and source-map the wasm via `wasm-bindgen --keep-debug` so stack traces become readable.
2. Add explicit `tracing::info!` markers throughout `crate::sync::matview_manager::ensure_view` and the post-DDL hook path.
3. Look for `unreachable!()` macros and `unwrap()` on a `None` in the matview / view subscription path.

The "1 resource available" is `watch_view_d77ac41ba85c1706` — find which preloaded view that is (likely the root layout query) and trace its `subscribe_cdc` path.

## Files touched in this devlog

- `frontends/dioxus-web/Cargo.toml` (new), `index.html` (new), `Trunk.toml` (new), `src/main.rs` (new)
- `Cargo.toml` — added `frontends/dioxus-web` workspace member
- `crates/holon/Cargo.toml` — wasm32 deps add `wasm-bindgen-futures`, `web-time`
- `crates/holon/src/util.rs` — added `now_unix_millis`, `MonotonicInstant`, `spawn_actor`
- `crates/holon/src/storage/turso.rs` — wasm32 cfg gate on `tokio::spawn`, swap `row_changes` to `spawn_actor`, add wasm32 `open_database` branch
- `crates/holon/src/sync/matview_manager.rs` — `tokio::spawn` → `spawn_actor`
- `crates/holon/src/di/lifecycle.rs`, `src/core/sql_operation_provider.rs`, `src/api/holon_service.rs`, `src/api/memory_backend.rs`, `src/api/loro_backend.rs` — std::time → util helpers
- `~/.cargo/git/checkouts/turso-.../{c4dc3fc,cdd46b1}/workspace-hack/Cargo.toml` — stubbed (out-of-tree, not committed)

## Definition-of-done checklist (handoff)

| Criterion | Status |
|-----------|--------|
| Static site loads without console errors | ✅ — release wasm boots clean in Chromium |
| `.wasm` ≤ 5 MB gzipped | ❌ — currently 87 MB unstripped (Phase 3) |
| Turso :memory: + Loro instantiate | ✅ — Turso confirmed via console traces, Loro untested |
| Renders at least one screen | ✅ — 3 seeded blocks rendered from in-memory Turso (static one-shot, not reactive `watch_ui`) |
| Interactive (click → operation → CDC update) | ✅ — click → INSERT only; new row arrives via real CDC stream and updates the UI; events_received counter advances |
| Refresh wipes state | n/a until something renders |
