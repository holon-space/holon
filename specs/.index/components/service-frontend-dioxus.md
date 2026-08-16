---
name: frontend-dioxus
description: Browser UI frontend built with Dioxus (WASM/CSR)
type: reference
source_type: component
source_id: frontends/dioxus-web/
category: service
fetch_timestamp: 2026-04-23
---

## frontends/dioxus-web

**Purpose**: Browser-targeting UI frontend using the Dioxus framework, compiled to WASM/CSR. Secondary priority behind GPUI. The former `frontends/dioxus/` SSR/desktop crate has been deleted.

### Crates

| Crate | Source | Target |
|-------|--------|--------|
| `holon-dioxus-web` | `frontends/dioxus-web/` (42 files) | WASM / browser |

### Related

- **holon-frontend**: provides shared `ReactiveViewModel` and session layer
- **holon-worker**: `wasm32-wasip1-threads` backend running as Web Worker alongside `dioxus-web`
