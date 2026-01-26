---
name: gpui
description: GPU-accelerated UI framework (Zed fork) powering holon's desktop and mobile UI
type: reference
source_type: url
source_id: https://github.com/holon-space/zed (branch: holon)
fetch_timestamp: 2026-04-23
---

## GPUI (holon-space/zed fork, branch: holon)

**Purpose**: GPU-accelerated hybrid immediate/retained-mode UI framework for Rust. Powers Zed editor and holon's desktop/mobile UI.

### Key APIs

| Type | Role |
|------|------|
| `Entity<T>` | GPUI-owned typed state reference |
| `WeakEntity<T>` | Weak reference; `.update()` returns `Result` (entity may be freed) |
| `AnyEntity` / `AnyView` | Type-erased dynamic dispatch |
| `Render` trait | Defines visual output; returns element tree |
| `AppContext` | Application-global state access |
| `WindowContext` | Per-window operations |
| `Context<T>` | Entity-specialized type-safe access |
| `flush_effects()` | Propagates state changes through element tree; triggers redraws |
| `run_until_parked()` | Runs event loop until no work remains (test utility) |
| `Div` / `InteractiveElement` | Layout + event handling primitives |

### Layout & Styling

- CSS-like properties via Tailwind-inspired API
- Taffy layout engine (flexbox)
- `.cached()` + `size_full()` pattern prevents idle render cascades

### Entity Lifecycle Notes (from holon memory)

- `flush_effects()` frees entities — not `run_until_parked()`
- Signal tasks must break on `WeakEntity::update()` returning `Err`
- `.cached()` is critical for avoiding 6000+ idle rerender cycles (fixed in holon)

### Integration in Holon

- **frontends/gpui**: `AppModel`, `FocusRegistry`, `NavigationState`, `BoundsRegistry`
- Entity-per-block reactive pattern: each visible block gets a GPUI entity driven by CDC streams
- Mobile support via `gpui-mobile` fork (feature-gated)
- Layout snapshot tests via `insta`; property-based tests via `proptest`

### Keywords
gpui, ui, rendering, entity, reactive, desktop, mobile, Zed, GPU
