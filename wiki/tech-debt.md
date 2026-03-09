---
title: Tech Debt & Architectural Inconsistencies
type: concept
tags: [tech-debt, architecture, refactoring]
created: 2026-04-14
updated: 2026-04-14
related_files: []
---

# Tech Debt & Architectural Inconsistencies

Living register of known structural problems. Update this page during INGEST whenever related code changes.
Severity: **High** = architectural confusion / likely source of bugs. **Medium** = friction / maintenance cost. **Low** = cleanup / cosmetic.

---

## High

### 1. `BackendEngine` vs `HolonService` — blurry responsibility boundary

`HolonService` (`crates/holon/src/api/holon_service.rs`) wraps `BackendEngine` and adds timing, `build_context()`, `list_tables()`, `undo()`/`redo()`. There is no documented rule for what belongs in each. `HolonService` is described as "used by both MCP and integration tests" — a sign it became a grab-bag adapter over time.

**Risk**: query/operation logic accumulates in whichever file is most convenient, not where it belongs.
**Fix direction**: define a clear contract — `BackendEngine` owns execution primitives; `HolonService` owns application-level workflows. Alternatively, collapse `HolonService` into `BackendEngine`.

---

### 2. `ReactiveEngine` violates single responsibility

`crates/holon-frontend/src/reactive.rs::ReactiveEngine` simultaneously:
- Implements `BuilderServices` (service locator for all builders)
- Manages watcher lifecycle (`HashMap<EntityUri, ReactiveQueryResults>`)
- Acts as top-level reactive coordinator

**Fix direction**: extract `WatcherRegistry` (lifecycle management) and keep `ReactiveEngine` as the `BuilderServices` impl. The registry becomes an internal dependency.

---

### 3. Two parallel builder systems must stay in sync manually

`holon-frontend/src/shadow_builders/` (~20 files, ViewModel builders) and `frontends/gpui/src/render/builders/` (~20 files, GPUI renderers) have a 1:1 correspondence. Adding a widget requires creating files in both directories. No compile-time enforcement of the pairing.

**Risk**: easy to add a shadow builder without its GPUI counterpart (or vice versa), producing a runtime panic in the dispatch table.
**Fix direction**: a registry macro or trait that links both sides at compile time, or a code generation step.

---

## Medium

### 4. Two coalescing layers with unclear distinction

- `TursoBackend::coalesce_row_changes()` in `crates/holon/src/storage/turso.rs` — merges DELETE+INSERT → UPDATE at the storage layer
- `coalesce()` in `crates/holon-api/src/reactive.rs` — stream-level coalescing utility

Whether these serve genuinely distinct purposes or partially overlap is not documented. If they duplicate logic, a double-coalesce bug is possible.

**Fix direction**: document the exact contract of each and add a comment cross-referencing them.

---

### 5. `AppModel` carries both a live and a static view of the same data

```rust
root_vm: ReactiveViewModel,   // live reactive tree
view_model: ViewModel,        // static snapshot produced from root_vm each render cycle
```

The snapshot `ViewModel` is used by MCP's `get_display_tree` and GPUI's render pass. The conversion on every update adds latency. It's unclear whether `ViewModel` offers anything that `ReactiveViewModel` cannot provide directly.

**Fix direction**: audit `ViewModel` usages and determine if the snapshot type can be eliminated.

---

### 6. `reactive.rs` appears in two crates with the same name, different concerns

- `crates/holon-api/src/reactive.rs` — generic stream utilities: `CdcAccumulator`, `ReactiveStreamExt`, `coalesce`, `materialize_map`
- `crates/holon-frontend/src/reactive.rs` — application coordinator: `ReactiveEngine`, `ReactiveQueryResults`

**Fix direction**: rename one. The `holon-api` file could become `stream_utils.rs` or `cdc_utils.rs`.

---

## Low

### 7. `created_at` schema type mismatch

DDL in `crates/holon/src/storage/schema_modules.rs` defines `created_at TEXT NOT NULL DEFAULT (datetime('now'))` but `Block` struct has `created_at: i64` (epoch millis). If a caller omits `created_at`, SQLite's DEFAULT fires and produces a string that silently breaks `Block` deserialization (now caught by `.expect()` in `rows_to_blocks`).

**Fix direction**: change DDL to `INTEGER NOT NULL DEFAULT (unixepoch('now', 'subsec') * 1000)` to match the Rust type.

---

### 8. Abandoned frontend prototypes still in the repo

`frontends/tui/`, `frontends/waterui/`, `frontends/ply/`, `frontends/dioxus/` exist alongside the active GPUI frontend and demoted Flutter frontend. These are past experiments that add noise to the dependency graph.

**Fix direction**: delete or move to an `archive/` branch. If kept, mark clearly in their `Cargo.toml` or README as archived.

---

## Related Pages

- [[overview]] — architecture overview
- [[entities/holon-frontend]] — shadow builders, ReactiveEngine, BuilderServices
- [[entities/gpui-frontend]] — GPUI render builders, AppModel
- [[entities/holon-crate]] — BackendEngine, HolonService
- [[concepts/reactive-view]] — ReactiveEngine design
- [[concepts/cdc-and-streaming]] — coalescing pipeline
