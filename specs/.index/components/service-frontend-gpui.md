---
name: frontend-gpui
description: GPUI desktop/mobile UI frontend — reactive shell, block rendering, navigation, entity views
type: reference
source_type: component
source_id: frontends/gpui/src/
category: service
fetch_timestamp: 2026-04-23
---

## frontends/gpui (holon-gpui)

**Purpose**: Primary desktop UI (macOS/Linux/Windows) and mobile UI (feature-gated) built with the GPUI framework. #1 frontend priority for holon.

### Key Modules

| Module | Role |
|--------|------|
| `views/reactive_shell` | `ReactiveShell` — main container; drives CDC → GPUI entity pipeline |
| `views/render_block_view` | `RenderBlockView` — renders a single block via `WidgetSpec` |
| `render/builders/` | Builder impls (view_mode_switcher, etc.) that produce GPUI elements |
| `navigation_state` | `NavigationState` — tracks current focus, breadcrumb, back/forward |
| `entity_view_registry` | `EntityViewRegistry` — maps entity names → GPUI view factories |
| `di` | FluxDI module wiring for GPUI session |
| `geometry` | Layout calculations, bounds tracking |
| `reactive_vm_poc` | Reactive ViewModel POC (in-progress refactor area) |
| `inspector` | Debug inspector overlay (debug builds only) |
| `mobile` | Mobile UI components (feature-gated: `mobile`) |
| `share_ui` | Share / accept UI flows |
| `user_driver` | User interaction event routing |

### Architecture Notes

- `ReactiveShell` must use `size_full()` (not `flex_1`) when parent panel is `absolute` (block-mode panels) — bug fix Apr 2026
- `.cached()` + `size_full()` layout chain is critical to prevent idle render cascade (6000 → 0 render cycles)
- Columns bypass `ReactiveShell` directly (performance optimization)
- GPUI entity lifecycle: `flush_effects()` frees entities; signal tasks break on `WeakEntity::update()` returning `Err`

### Testing

- Layout snapshot tests via `insta` (`holon-layout-testing`)
- Property-based tests via `proptest`
- E2E reproduction via `holon-integration-tests/tests/general_e2e_pbt.rs` (preferred first stop for UI bugs)

### Related

- **holon-frontend**: provides `ReactiveViewModel`, `ViewModel`, theming
- **frontends/mcp**: MCP server runs alongside GPUI session
- **holon-api**: `WidgetSpec`, `UiEvent`, `RenderExpr` consumed here
