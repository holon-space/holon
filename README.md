# Holon

> *"Reality is not composed of things or processes, but of holons."* — Ken Wilber

**Holon** is an offline-first, Rust-based personal knowledge and task management system. It treats external systems (org-mode files, Todoist, JIRA, calendars) as first-class data sources with bidirectional sync, unified queries, and reactive UI updates — all backed by CRDTs and an embedded SQL cache.

**Website**: [holon.space](https://holon.space)

## Why Holon?

Modern knowledge workers juggle tasks across Todoist, JIRA, Linear; notes across Notion, Obsidian, LogSeq; calendars, email, files — each in its own silo. This fragmentation destroys flow. Holon eliminates it by integrating all your systems into a unified view where:

- You see everything that matters in one place
- You trust that nothing is forgotten
- AI helps you see patterns and connections you'd miss
- You achieve flow states more frequently

Holon is a **trust and flow system** that happens to use productivity data. For the full philosophical foundation, see [VISION_LONG_TERM.md](VISION_LONG_TERM.md).

## Architecture Overview

```
User Action → Operation Dispatch → External/Internal System
                                          ↓
UI ← CDC Stream ← QueryableCache ← Sync Provider
```

Operations are fire-and-forget. Effects are observed through reactive CDC streams. Internal and external modifications are treated identically.

```
┌─────────────────────────────────────────────────────────┐
│              UNIFIED TURSO CACHE                        │
│         (SQLite-compatible, single query surface)       │
│    All data—owned and third-party—queryable here        │
└─────────────┬─────────────────────────┬─────────────────┘
              │                         │
      ┌───────▼───────┐         ┌───────▼───────────┐
      │  LORO CRDT    │         │  SYNC PROVIDERS   │
      │  Source of    │         │  (org-mode files,  │
      │  truth for    │         │   Todoist API,     │
      │  owned data   │         │   MCP clients)     │
      └───────────────┘         └───────────────────┘
```

Both owned data (Loro CRDT) and third-party data flow into the same Turso cache. The UI queries this single unified surface using PRQL, GQL, or raw SQL. Rendering is specified separately in Rhai-based render expressions.

For the full architecture, see [docs/Architecture.md](docs/Architecture.md).

## Crate Structure

```
crates/
├── holon/                    # Main orchestration: Turso cache, CDC, query engine
├── holon-api/                # Shared types, operations, change descriptors, session capability traits
├── holon-app/                # Wiring crate: DI assembly naming concrete backends
├── holon-core/               # Core traits: DataSource, CrudOperations, BlockOperations
├── holon-engine/             # Standalone Petri-net engine CLI (YAML nets, WSJF ranking)
├── holon-frontend/           # Platform-agnostic ViewModel layer (MVVM)
├── holon-macros/             # Procedural macros (#[operations_trait], entity derives)
├── holon-macros-test/        # Macro expansion tests
├── holon-mcp-client/         # MCP client → OperationProvider bridge
├── holon-todoist/            # Todoist API integration
├── holon-orgmode/            # Org-mode file parsing, sync via file watching
├── holon-filesystem/         # File system directory integration
└── holon-integration-tests/  # Cross-crate integration & property-based tests

frontends/
├── gpui/         # GPUI frontend (primary)
├── mcp/          # MCP server frontend (stdio + HTTP)
├── ply/          # Ply frontend
├── tui/          # Terminal UI frontend
├── dioxus/       # Dioxus frontend
├── dioxus-web/   # Dioxus web frontend (wasm32, trunk build)
├── holon-worker/ # Holon backend as a Web Worker (wasm32-wasip1-threads)
└── waterui/      # WaterUI frontend
```

Archived prototypes (Flutter, Blinc) are not in the tree; they live on the
`frontends/flutter` and `frontends/blinc` bookmarks.

## Key Concepts

### Multi-Language Queries

Data queries use PRQL (primary), GQL (ISO/IEC 39075 graph queries), or raw SQL. Rendering is specified in a sibling render block using Rhai syntax:

```org
#+BEGIN_SRC holon_prql
from children
select {id, content, content_type, source_language}
#+END_SRC
#+BEGIN_SRC render
list(#{item_template: render_block()})
#+END_SRC
```

### Org-Mode as First-Class Data Source

Org files are bidirectionally synced: edit in Emacs/Vim/any editor, changes appear in Holon; edit in Holon, changes are written back to disk. The `OrgSyncController` handles echo suppression to prevent sync loops.

> **⚠️ Vault + file syncers:** see [Vault directory & third-party file sync](#vault-directory--third-party-file-sync-important).

### Petri-Net Engine & WSJF Ranking

Tasks are materialized into a Petri Net model with typed tokens (Person, Organization, Document, Monetary, Knowledge, Resource). The engine computes WSJF (Weighted Shortest Job First) rankings using prototype blocks with Rhai-evaluated scoring expressions. See [VISION_PETRI_NET.md](VISION_PETRI_NET.md).

### MCP Server

Every frontend automatically launches an MCP server, exposing the full query surface, UI inspection, undo/redo, navigation, and org file rendering to AI agents.

### Structural Primacy

Intelligence resides in the data structure, not in the AI model. The substitution test: swap the AI model — the system still works. Remove the data structure — nothing can reconstruct it. Schemas, typed relationships, materialized views, and the Petri Net are all structural intelligence. See [VISION_AI.md](VISION_AI.md).

## Building

### Prerequisites

- Rust (see `rust-toolchain.toml` for the exact version)

### Build

```bash
cargo build
```

### Test

Tests use real database connections and must run sequentially:

```bash
cargo test --tests -- --test-threads=1
```

See [TESTING.md](TESTING.md) for details.

### Run the MCP Server

```bash
cargo run -p holon-mcp
```

### Run the Petri-Net Engine CLI

```bash
cargo run -p holon-engine -- --help
```

### Vault directory & third-party file sync (important)

Do **not** place a Holon vault (`HOLON_VAULT_ROOT`) under Syncthing, iCloud
Drive, Dropbox, or any byte-level file syncer while Loro CRDT sync is enabled.
Holon converges devices through Loro/P2P; a second, block-unaware syncer
writing the same files makes foreign echoes indistinguishable from user edits
(duplicated edits, sibling-order churn, conflict-copy files ingested as
duplicate documents). One vault, one syncer: Holon. See
[docs/Architecture/Model.md](docs/Architecture/Model.md), invariant 11.

## Vision Documents

| Document | Contents |
|----------|----------|
| [docs/Vision.md](docs/Vision.md) | Technical vision & phased roadmap |
| [docs/Vision/LongTerm.md](docs/Vision/LongTerm.md) | Philosophical foundation: Integral Theory, flow psychology, the Holon promise |
| [docs/Vision/AI.md](docs/Vision/AI.md) | Three AI roles (Watcher, Integrator, Guide), trust ladder, privacy model |
| [docs/Vision/PetriNet.md](docs/Vision/PetriNet.md) | Petri-Net primitives, Digital Twins, WSJF sorting |
| [docs/Vision/UI.md](docs/Vision/UI.md) | UI/UX design system: three modes (Capture, Orient, Flow), color palette, micro-interactions |
| [docs/Architecture.md](docs/Architecture.md) | Full technical architecture: traits, data flow, CDC, entity types (details in [docs/Architecture/](docs/Architecture/)) |

## Core Dependencies

- **Loro** — CRDT library for collaborative/offline-first data
- **Turso** (libSQL) — Embedded SQLite cache with incremental view maintenance
- **Tokio** — Async runtime
- **PRQL** — Pipelined Relational Query Language, compiles to SQL
- **Rhai** — Embedded scripting for render expressions and Petri-net guards

## License

See [LICENSE](LICENSE).
