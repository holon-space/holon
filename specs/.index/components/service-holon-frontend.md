---
name: holon-frontend
description: Reactive UI framework layer: ReactiveViewModel, SessionConfig, theming, navigation, value functions
type: reference
source_type: component
source_id: crates/holon-frontend/src/
category: service
fetch_timestamp: 2026-04-23
---

## holon-frontend (crates/holon-frontend)

**Purpose**: UI-agnostic reactive layer between backend engine and concrete UI frameworks (GPUI, Dioxus, TUI). Manages reactive view model, session config, theming, navigation, and input handling.

### Key Modules

| Module | Role |
|--------|------|
| `reactive_view_model` | `ReactiveViewModel` — persistent-node architecture, drives UI via CDC streams |
| `reactive_view` | `ReactiveView` — component tree nodes |
| `view_model` | `ViewModel` — snapshot of reactive tree (immutable, sent to renderer) |
| `navigation` | Navigation cursor, history, focus management |
| `config` | `HolonConfig`, `SessionConfig`, `UiConfig` — TOML-based configuration |
| `theme` | `ThemeRegistry`, theme switching |
| `input` | `InputAction`, `Key`, `WidgetInput` — input event dispatch |
| `input_trigger` | Input triggering system |
| `operations` | Operation execution pipeline |
| `value_fns` | Computed value functions (vfn1–vfn13 and beyond) |
| `provider_cache` | `ProviderCache` — memoized value function results |
| `render_interpreter` | Render DSL evaluation |
| `shadow_builders` | Shadow DOM / virtual tree builders |
| `shadow_index` | Shadow index management |
| `cdc` | CDC event integration (Turso → reactive signals) |
| `mcp_integrations` | MCP client integration for live data |
| `editor_controller` | Text editor integration (content editing) |
| `preferences` | User preferences |
| `logging` | Tracing/observability hooks |
| `memory_monitor` | Memory profiling utilities |
| `widget_gallery` | Widget component gallery |

### ReactiveViewModel Architecture

- **Persistent-node architecture** (post Apr-2026 refactor): nodes persist across renders; only diffs applied
- Entity cache (`entity_cache`) must persist between renders — do NOT recreate per render
- DAG cycle count reduced from 6864 → 9 via architectural fix
- Columns bypass ReactiveShell directly (optimization)

### Session Flow

```
HolonConfig → SessionConfig → FrontendSession → ReactiveViewModel → ViewModel → UI render
```

### Related

- **frontends/gpui**: consumes `ReactiveViewModel` / `ViewModel`
- **holon**: `BackendEngine` drives CDC events consumed here
- **holon-api**: all streaming types come from here
