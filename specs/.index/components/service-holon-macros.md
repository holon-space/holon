---
name: holon-macros
description: Procedural macros for entity derivation, operations traits, and widget builder code generation
type: reference
source_type: component
source_id: crates/holon-macros/src/
category: service
fetch_timestamp: 2026-04-23
---

## holon-macros (crates/holon-macros)

**Purpose**: proc-macro crate generating boilerplate for entity types, operation trait impls, and widget builder registrations.

### Macros

| Macro | Role |
|-------|------|
| `#[derive(Entity)]` | Generates `FromEntity`, `ToEntity`, SQL column mapping, and `DataSource` impls |
| `#[operations_trait]` | Generates trait + dispatch impl for operation registries |
| Widget builder macros | Code generation for `BuilderRegistry` entries |

### Key Modules

| Module | Role |
|--------|------|
| `entity` | `#[derive(Entity)]` implementation |
| `attr_parser` | Parses `#[entity(...)]` attributes |
| `operations_trait` | `#[operations_trait]` implementation |
| `builder_registry` | Widget builder registration code generation |
| `widget_builder` | Widget builder macro support |

### Schema Mismatch Warning

`#[derive(Entity)]` relies on `created_at: i64` but DDL uses `TEXT NOT NULL DEFAULT (datetime('now'))`. Any block creation must provide explicit `Value::Integer(millis)` or parsing panics (`.expect()` in `rows_to_blocks`).

### Related

- **holon-macros-test**: integration test crate for macro output verification
- Used by: all crates that define entity types (holon-api, holon-todoist, etc.)
