---
name: frontend-other
description: Secondary frontends: TUI, WaterUI, Ply, Flutter, holon-worker (WASM)
type: reference
source_type: component
source_id: frontends/
category: service
fetch_timestamp: 2026-04-23
---

## Other Frontends

### holon-tui (`frontends/tui`, 27 files)
Terminal UI using TUI frameworks. Secondary priority.

### holon-waterui (`frontends/waterui`, 24 files)
Cross-platform UI framework. Secondary priority.

### holon-ply (`frontends/ply`, 38 files)
Custom UI framework / playground.

### frontends/flutter
Flutter mobile UI (Dart). Demoted to second-tier due to FRB debugging friction. Not the primary mobile path (GPUI mobile feature-gate is preferred).

### holon-worker (`frontends/holon-worker`, 4 files)
WASM Web Worker target (`wasm32-wasip1-threads`). Runs the holon backend in a browser Web Worker thread alongside Dioxus-web frontend.

### Related

- **holon-frontend**: shared reactive layer consumed by all frontends
- **GPUI**: primary frontend — see `service-frontend-gpui.md`
