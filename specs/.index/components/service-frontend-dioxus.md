---
name: frontend-dioxus
description: Web UI frontends built with Dioxus framework (SSR/CSR/WASM)
type: reference
source_type: component
source_id: frontends/dioxus/ + frontends/dioxus-web/
category: service
fetch_timestamp: 2026-04-23
---

## frontends/dioxus + frontends/dioxus-web

**Purpose**: Web-targeting UI frontends using the Dioxus framework. `dioxus` targets SSR/desktop, `dioxus-web` targets WASM/CSR. Secondary priority behind GPUI.

### Crates

| Crate | Source | Target |
|-------|--------|--------|
| `holon-dioxus` | `frontends/dioxus/` (29 files) | SSR / desktop |
| `holon-dioxus-web` | `frontends/dioxus-web/` (42 files) | WASM / browser |

### Related

- **holon-frontend**: provides shared `ReactiveViewModel` and session layer
- **holon-worker**: `wasm32-wasip1-threads` backend running as Web Worker alongside `dioxus-web`
