# Holon Codebase Index

Generated: 2026-04-23

## Overview

Holon is a reactive personal knowledge management (PKM) system built in Rust. It stores data in Turso (SQLite fork with IVM/CDC), synchronizes with org-mode files via Loro CRDT, and renders through multiple UI frontends (GPUI primary, Dioxus web, TUI, Flutter mobile). An MCP server exposes the backend to AI agents.

**Workspace**: 15 crates + 9 frontends

---

## Component Specs (15 components)

### Backend Crates (`crates/`)

| Spec | Crate | Summary |
|------|-------|---------|
| [service-holon.md](components/service-holon.md) | `holon` | Root backend: Turso storage, Loro sync, type registry, Petri net wiring, render DSL |
| [service-holon-api.md](components/service-holon-api.md) | `holon-api` | Shared types: Block, EntityUri, RenderExpr, UiEvent, streaming types |
| [service-holon-core.md](components/service-holon-core.md) | `holon-core` | Core traits: CRUD, BlockOperations, DataSource, UndoStack, FractionalIndex |
| [service-holon-engine.md](components/service-holon-engine.md) | `holon-engine` | Petri net state machine with Rhai guard expressions |
| [service-holon-frontend.md](components/service-holon-frontend.md) | `holon-frontend` | Reactive UI layer: ReactiveViewModel, SessionConfig, theming, navigation, value functions |
| [service-holon-filesystem.md](components/service-holon-filesystem.md) | `holon-filesystem` | Filesystem abstraction, directory watching, DirectoryDataSource |
| [service-holon-macros.md](components/service-holon-macros.md) | `holon-macros` | Proc macros: `#[derive(Entity)]`, `#[operations_trait]`, widget builder codegen |
| [service-holon-org-format.md](components/service-holon-org-format.md) | `holon-org-format` | Pure org-mode parsing, rendering, diffing (no I/O) |
| [service-holon-orgmode.md](components/service-holon-orgmode.md) | `holon-orgmode` | Org-mode disk I/O, bidirectional sync controller, file watching |
| [service-holon-todoist.md](components/service-holon-todoist.md) | `holon-todoist` | Todoist task sync with real HTTP client and fake for testing |
| [service-holon-mcp-client.md](components/service-holon-mcp-client.md) | `holon-mcp-client` | MCP client: resource discovery, schema mapping, FDW virtual tables |

### Frontend Crates (`frontends/`)

| Spec | Frontend | Summary |
|------|----------|---------|
| [service-frontend-gpui.md](components/service-frontend-gpui.md) | `gpui` | **Primary** — GPUI desktop/mobile UI; ReactiveShell, block rendering, navigation |
| [service-frontend-mcp.md](components/service-frontend-mcp.md) | `mcp` | MCP server exposing backend tools + resources to AI agents |
| [service-frontend-dioxus.md](components/service-frontend-dioxus.md) | `dioxus` + `dioxus-web` | Web UI via Dioxus (SSR/CSR/WASM) |
| [service-frontend-other.md](components/service-frontend-other.md) | TUI / WaterUI / Ply / Flutter / holon-worker | Secondary frontends |

### Test Crates (`crates/`)

| Spec | Crate | Summary |
|------|-------|---------|
| [service-holon-integration-tests.md](components/service-holon-integration-tests.md) | `holon-integration-tests` | **Primary E2E PBT** — general_e2e_pbt.rs; real Turso DB; first stop for bug reproduction |
| [service-holon-layout-testing.md](components/service-holon-layout-testing.md) | `holon-layout-testing` | GPUI layout snapshots (insta) + proptest visual regressions |
| [service-holon-architecture-tests.md](components/service-holon-architecture-tests.md) | `holon-architecture-tests` | Architectural rule enforcement (crate layering) |

---

## External Resource Specs (6 resources)

| Spec | Resource | Summary |
|------|----------|---------|
| [url-futures-signals.md](external/url-futures-signals.md) | futures-signals v0.3 | FRP reactive streaming (Signal, MutableVec) |
| [url-loro.md](external/url-loro.md) | Loro v1.0 | CRDT framework for org-mode sync |
| [url-gpui.md](external/url-gpui.md) | GPUI (holon-space/zed fork) | GPU-accelerated UI framework |
| [url-fluxdi.md](external/url-fluxdi.md) | FluxDI (holon-space/fluxdi) | Type-safe DI with async factory + lifecycle |
| [url-turso.md](external/url-turso.md) | Turso (nightscape fork) | SQLite + IVM + CDC + MVCC storage backend |
| [mcp-holon.md](external/mcp-holon.md) | Holon MCP server | 30+ tools for DB, streaming, ops, debugging, org, UI |

---

## Key Architecture Patterns

| Pattern | Where |
|---------|-------|
| **Operations Registry** | `holon-macros` `#[operations_trait]` + `holon-core` traits |
| **Reactive CDC Streaming** | Turso IVM → `CacheEventSubscriber` → `ReactiveViewModel` → GPUI |
| **DI Composition** | FluxDI modules wired at startup (CoreInfra, Loro, EventInfra) |
| **Undo/Redo** | `UndoStack` wrapping `OperationLogOperations` |
| **DataSource Abstraction** | Trait-based over Turso, in-memory, Todoist, OrgMode, MCP, Filesystem |
| **Petri Net Workflows** | `holon-engine` YAML nets with Rhai guards |
| **Type Registry** | Pluggable entity profiles with computed fields and render DSL |
| **Render DSL** | Rhai-based expression trees (RenderExpr) compiled once, evaluated per render |
| **CRDT Sync** | Loro LoroDoc drives org-mode bidirectional sync with echo suppression |
| **Fail Loud** | `.expect()` not `.ok()`, never swallow errors, no silent fallbacks |

---

## Codebase Stats

| Category | Count |
|----------|-------|
| Backend crates | 11 |
| Frontend crates | 9 (4 tracked here) |
| Test crates | 3 (all tracked) |
| Component specs | 18 |
| External resource specs | 6 |
| **Total specs** | **24** |

---

## Next Steps

Run `/ralph-specum:start` to create feature specs that reference these indexed components.
