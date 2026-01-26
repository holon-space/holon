---
title: Architecture Overview
type: overview
tags: [architecture, tech-stack, directory-layout]
created: 2026-04-13
updated: 2026-04-13
related_files: [README.md, docs/Architecture.md, Cargo.toml]
---

# Architecture Overview

## What is Holon?

Holon is an **offline-first, Rust-based Personal Knowledge & Task Management (PKM) system**. Its name is from Ken Wilber's philosophy: reality is made of holons (things that are both wholes and parts). The system treats external data sources (org-mode files, Todoist, JIRA, calendars) as first-class citizens with bidirectional sync, unified queries, and reactive UI updates.

The central design insight: **external systems are sync providers**, not import targets. Edits to an org file on disk appear live in Holon; edits in Holon are written back to the org file.

## Core Data Flow

```
User Action → Operation Dispatch → External/Internal System
                                         ↓
UI ← CDC Stream ← QueryableCache ← Sync Provider
```

Operations are **fire-and-forget**. Effects are observed through reactive CDC (Change Data Capture) streams. Internal and external modifications flow through the same pipeline identically.

## Storage Architecture

```
┌─────────────────────────────────────────────┐
│         UNIFIED TURSO CACHE                 │
│   All data (owned and third-partyr lives here  │
└──────────────┬──────────────────────────────┘
               │
   ┌───────────┴───────────┐
   ▼                       ▼
┌──────────────┐    ┌──────────────────┐
│  LORO CRDT   │    │  SYNC PROVIDERS  │
│  (owned data │    │  (org files,     │
│   source of  │    │   Todoist API,   │
│   truth)     │    │   MCP clients)   │
└──────────────┘    └──────────────────┘
```

- **Turso DB** — embedded SQLite-compatible DB, all data is queryable here
- **Loro CRDT** — source of truth for owned (authored) data; changes flow from Loro → Turso cache
- **Sync providers** — read/write external systems; changes flow as `Change<T>` into `QueryableCache`

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Runtime | Tokio (async) |
| Storage | Turso DB with IVM (Incremental View Maintenance) |
| CRDT | Loro — collaborative offline-first data |
| Query languages | PRQL → SQL, GQL (ISO/IEC 39075 graph) → SQL, raw SQL |
| Render DSL | Rhai (embedded scripting) |
| Petri net guards | Rhai |
| Reactive signals | futures-signals (`Mutable`, `MutableVec`, `MutableBTreeMap`) |
| Primary frontend | GPUI (native GPU-accelerated Rust UI) |
| Secondary frontend | Flutter (mobile/web) via FFI bridge |
| AI/tool interface | MCP (Model Context Protocol) server |
| DI container | fluxdi |
| Proc macros | custom `holon-macros` crate |

## Directory Layout

```
holon/
├── crates/
│   ├── holon/               # Main orchestration: storage, sync, API, DI
│   ├── holon-api/           # Shared types (Value, Block, RenderExpr, streaming)
│   ├── holon-core/          # Core traits (DataSource, CrudOperations, BlockOperations)
│   ├── holon-engine/        # Standalone Petri-net engine CLI (YAML nets, WSJF)
│   ├── holon-frontend/      # Platform-agnostic ViewModel layer (MVVM)
│   ├── holon-macros/        # Proc macros (#[Entity], #[operations_trait], builder_registry)
│   ├── holon-macros-test/   # Macro expansion tests
│   ├── holon-mcp-client/    # MCP client → OperationProvider bridge
│   ├── holon-orgmode/       # Org-mode file parsing, rendering, and bidirectional sync
│   ├── holon-todoist/       # Todoist API integration
│   ├── holon-filesystem/    # File system directory provider
│   └── holon-integration-tests/ # Cross-crate PBT integration tests
├── frontends/
│   ├── gpui/                # Primary GPUI frontend
│   ├── flutter/             # Flutter/Dart frontend with FFI bridge
│   ├── mcp/                 # MCP server frontend (stdio + HTTP)
│   ├── tui/                 # Terminal UI
│   ├── waterui/             # WaterUI prototype
│   ├── ply/                 # Ply frontend
│   └── dioxus/              # Dioxus (web/mobile) frontend
├── docs/                    # Developer docs (ORG_SYNTAX.md, etc.)
├── scripts/                 # Log analysis scripts (PM4Py, Drain3, metrics)
├── sql/                     # SQL schema files (blocks.sql, etc.)
└── wiki/                    # This wiki
```

## Key Design Decisions

### 1. Parse, Don't Validate

Types encode invariants at parse boundaries. Raw strings (`parent_id`, entity URIs) are parsed into `EntityUri` at the boundary; downstream code uses typed values. See [[concepts/value-type]] and `crates/holon-api/src/entity_uri.rs`.

### 2. Fail Loud, Never Fake

No `unwrap_or_default()` on errors, no silent fallbacks. Errors surface as panics or `Result::Err`. See `CLAUDE.md` §Error Handling Philosophy.

### 3. Structural Primacy

Intelligence lives in the data structure (schemas, typed relationships, materialized views, Petri net). Not in the AI model. The system must work if you swap the AI model; it cannot work if you delete the structure.

### 4. Reactive CDC Pipeline

The UI never polls. Data flows as CDC events from Turso IVM (Incremental View Maintenance) through `RowChangeStream` → `ReactiveQueryResults` → `futures-signals` reactive tree → GPUI entity updates.

### 5. Operations are Fire-and-Forget

All mutation entry points (indent, move, set_field, etc.) return immediately. Confirmation comes through CDC. This keeps the UI snappy and decoupled from external system latency.

### 6. Decoupled Org Sync

`OrgSyncController` uses a projection+diff pattern with echo suppression. It tracks the last-written content per file (`last_projection`) and compares disk content against it to distinguish external edits from its own writes, without timing windows. See [[entities/holon-orgmode]] and [[concepts/org-sync]].

## Crate Dependency Graph (simplified)

```
holon-api ← holon-core ← holon ← holon-orgmode
                    ↑              ↑
              holon-engine    holon-todoist
                    ↑
              holon-frontend ← frontends/gpui
                                frontends/mcp
                                frontends/flutter
```

`holon-api` has no frontend deps and is shared by all crates. `holon-frontend` adds the ViewModel/reactive layer consumed by all UIs.

## Logging

App logs go to `/tmp/holon.log` via `tracing`. JSON structured logging is available via `HOLON_LOG=file:///tmp/holon.json:json`. Analysis scripts in `scripts/` cover process mining (PM4Py), log clustering (Drain3), and metric sparklines.

## Related Pages

- [[entities/holon-crate]] — main orchestration crate
- [[entities/holon-api]] — shared type library
- [[entities/holon-frontend]] — MVVM reactive layer
- [[entities/gpui-frontend]] — GPUI primary frontend
- [[concepts/cdc-and-streaming]] — how data flows to the UI
- [[concepts/reactive-view]] — reactive ViewModel system
- [[concepts/query-pipeline]] — PRQL/GQL/SQL compilation
- [[concepts/petri-net-wsjf]] — task ranking engine
- [[concepts/org-sync]] — bidirectional org-mode sync
