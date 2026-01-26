---
title: Wiki Operation Log
type: concept
tags: [meta, log]
created: 2026-04-13
updated: 2026-04-13
related_files: []
---

# Wiki Operation Log

Append-only. One entry per significant wiki operation.

---

## 2026-04-14 — Tech debt page added

- Created `tech-debt.md` with 8 documented issues (3 High, 3 Medium, 2 Low)
- Added link to index.md Start Here section
- Issues: BackendEngine/HolonService boundary, ReactiveEngine SRP, parallel builder systems, double coalescing, AppModel dual view, reactive.rs naming collision, created_at schema mismatch, abandoned frontends

---

## 2026-04-13 — INIT

- Performed initial wiki creation from codebase exploration.
- Read: README.md, ARCHITECTURE.md, DEVELOPMENT.md, CLAUDE.md
- Explored all crates under `crates/` and frontends under `frontends/`
- Created: index.md, log.md, schema.md, overview.md
- Created entities/: holon-crate, holon-api, holon-core, holon-frontend, holon-orgmode, holon-engine, holon-macros, holon-integration-tests, gpui-frontend, mcp-frontend
- Created concepts/: cdc-and-streaming, reactive-view, org-sync, petri-net-wsjf, query-pipeline, entity-profile, value-type, pbt-testing, di-and-di-modules, loro-crdt
