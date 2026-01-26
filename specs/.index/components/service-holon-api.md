---
name: holon-api
description: Core type definitions: Block, EntityUri, RenderExpr, UiEvent, streaming types
type: reference
source_type: component
source_id: crates/holon-api/src/
category: service
fetch_timestamp: 2026-04-23
---

## holon-api (crates/holon-api)

**Purpose**: Shared type library — no logic, only domain types. All other crates depend on this; it has minimal dependencies itself.

### Key Modules & Types

| Module | Key Types |
|--------|-----------|
| `block` | `Block`, `BlockContent`, `BlockMetadata`, `TaskState`, `Priority` |
| `entity` | `EntityName`, entity macro support |
| `entity_uri` | `EntityUri` — parsing with scheme (`block:`, `doc:`, `file:`) |
| `render_types` | `RenderExpr`, `RenderVariant`, `RenderExpr::to_rhai()`, `visible_columns()` |
| `streaming` | `UiEvent`, `MapChange`, `BatchMapChange`, `StreamPosition`, `ChangeOrigin` |
| `widget_spec` | `WidgetSpec` — `render_expr: RenderExpr` + `data: Vec<ResolvedRow>` + `actions` |
| `reactive` | Reactive streaming type aliases |
| `input_types` | `Key`, keyboard chord types |
| `interp_value` | `InterpValue` — runtime interpreter values |
| `predicate` | `Predicate` — query predicate DSL |
| `render_eval` | Expression evaluation types |
| `link_parser` | Org-mode link parsing |
| `widget_meta` | Widget metadata registry |
| `auth` | `AuthProviderStatus` |

### Org File Bare ID Convention

`EntityUri` stores IDs WITHOUT scheme prefixes in org files. Parser adds scheme at boundary (`EntityUri::block(id)` for source blocks, `EntityUri::from_raw(id)` for headings). Renderer strips scheme (writes `.id()` not `.as_str()`).

### UiEvent Stream Protocol

```
UiEvent::Structure { widget_spec, generation }
UiEvent::Data { batch, generation }
```
Generation tracking: stale events discarded. Error recovery: render failures emit `Structure { error_widget_spec, gen++ }` — stream stays open.

### Related

- depended on by: all other crates
- **holon**: implements and uses these types in storage/API layer
