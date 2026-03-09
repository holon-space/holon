---
title: Holon Wiki Index
type: overview
tags: [index, meta]
created: 2026-04-13
updated: 2026-04-13
related_files: []
---

# Holon Wiki

Living documentation for the `holon` PKM system. Source code is the source of truth — this wiki is a navigational layer.

See [[schema]] for wiki conventions. See [[log]] for change history.

---

## Start Here

- [[overview]] — architecture overview, tech stack, directory layout, key decisions
- [[tech-debt]] — known architectural inconsistencies and structural debt (updated on each INGEST)

---

## Entities (Crates & Frontends)

| Page | Summary |
|------|---------|
| [[entities/holon-crate]] | Main orchestration crate: storage, sync, BackendEngine, watch_ui, Petri net materialization |
| [[entities/holon-api]] | Shared type library: Value, Block, EntityUri, RenderExpr, UiEvent, Change |
| [[entities/holon-frontend]] | Platform-agnostic MVVM layer: ReactiveEngine, ReactiveView, shadow builders |
| [[entities/gpui-frontend]] | Primary GPUI native frontend: AppModel, ReactiveShell, EditorView |
| [[entities/mcp-frontend]] | MCP server frontend: AI tool access, execute_query, get_display_tree |
| [[entities/holon-orgmode]] | Org-mode bidirectional sync: OrgSyncController, parser, renderer |
| [[entities/holon-engine]] | Standalone Petri-net engine: generic net execution, WSJF ranking, YAML CLI |
| [[entities/holon-macros]] | Procedural macros: #[Entity] derive, #[operations_trait], builder_registry |
| [[entities/holon-integration-tests]] | PBT infrastructure: state machine, ReferenceState, invariants, E2ETransition |

---

## Concepts (Architecture & Patterns)

| Page | Summary |
|------|---------|
| [[concepts/cdc-and-streaming]] | Change Data Capture pipeline: Turso IVM → RowChangeStream → UiEvent → frontend |
| [[concepts/reactive-view]] | Reactive ViewModel system: ReactiveQueryResults, ReactiveView, ReactiveViewModel, futures-signals |
| [[concepts/query-pipeline]] | Multi-language query compilation: PRQL → SQL, GQL → SQL, virtual tables, render DSL |
| [[concepts/petri-net-wsjf]] | WSJF task ranking via Petri net: prototype blocks, Rhai scoring, canary blocks |
| [[concepts/org-sync]] | Bidirectional org-mode sync: echo suppression, projection+diff, file conventions |
| [[concepts/loro-crdt]] | Loro CRDT: global LoroTree, offline-first, P2P sync, alias registry |
| [[concepts/entity-profile]] | Runtime per-row render resolution: EntityProfile, RenderVariant, Predicate conditions |
| [[concepts/value-type]] | Value enum, EntityUri, parse-don't-validate, StorageEntity, TypeDefinition |
| [[concepts/pbt-testing]] | Property-based testing strategy: ReferenceState, invariants, tee-before-filter rule |
| [[concepts/di-and-modules]] | Dependency injection: fluxdi, SchemaModule, startup flow, test helpers |

---

## Key Files Quick Reference

| File | Purpose |
|------|---------|
| `crates/holon/src/api/backend_engine.rs` | Central query/operation hub |
| `crates/holon/src/api/ui_watcher.rs` | `watch_ui()` — streaming reactive UI per block |
| `crates/holon/src/api/holon_service.rs` | `HolonService` — shared layer for MCP + tests |
| `crates/holon/src/storage/turso.rs` | `TursoBackend` actor + CDC broadcast |
| `crates/holon/src/petri.rs` | Task → Petri Net materialization |
| `crates/holon/src/di/lifecycle.rs` | `create_backend_engine()` startup |
| `crates/holon-api/src/lib.rs` | `Value`, re-exports of all shared types |
| `crates/holon-api/src/streaming.rs` | `UiEvent`, `WatchHandle`, `ChangeOrigin` |
| `crates/holon-api/src/render_types.rs` | `RenderExpr`, `RenderProfile`, `OperationDescriptor` |
| `crates/holon-frontend/src/reactive.rs` | `ReactiveEngine`, `BuilderServices`, `ReactiveQueryResults` |
| `crates/holon-frontend/src/reactive_view.rs` | `ReactiveView` self-managing pipeline |
| `crates/holon-orgmode/src/org_sync_controller.rs` | `OrgSyncController` with echo suppression |
| `crates/holon-orgmode/src/org_renderer.rs` | `OrgRenderer` — only path for org text generation |
| `crates/holon-engine/src/engine.rs` | `Engine::rank()` — WSJF ranking |
| `crates/holon-integration-tests/src/pbt/` | PBT state machine + invariants |
| `frontends/gpui/src/lib.rs` | `AppModel`, GPUI app structure |
| `frontends/mcp/src/tools.rs` | All MCP tool implementations |

---

## Logging & Debugging

- Logs: `/tmp/holon.log` (tracing crate format)
- JSON logs: `HOLON_LOG=file:///tmp/holon.json:json`
- Analysis: `scripts/analyze-log-pm4py.py`, `analyze-log-drain3.py`, `analyze-log-metrics.py`
- Always `tee` before filtering: `cmd 2>&1 | tee /tmp/out.log | grep FAIL`

## Testing

```bash
# PBT E2E tests
cargo nextest run -p holon-integration-tests general_e2e_pbt 2>&1 | tee /tmp/pbt.log

# Coverage
cargo llvm-cov --test general_e2e_pbt -p holon-integration-tests --html --output-dir target/coverage-report
```
