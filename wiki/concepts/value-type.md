---
title: Value Type & Parse-Don't-Validate
type: concept
tags: [types, value, parse-dont-validate, entity-uri, ffi]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon-api/src/lib.rs
  - crates/holon-api/src/entity_uri.rs
  - crates/holon-api/src/entity.rs
  - crates/holon-api/src/types.rs
---

# Value Type & Parse-Don't-Validate

## Parse, Don't Validate

From `PARSE_DONT_VALIDATE_AUDIT.md` and `CLAUDE.md`: make illegal states unrepresentable. Parse raw data into typed representations at entry points, never re-validate at call sites.

### EntityUri

`crates/holon-api/src/entity_uri.rs` — the canonical typed URI for all entities in Holon.

Schemes: `block:`, `doc:`, `file:`, `todo:`, `dir:`.

```rust
// AT THE BOUNDARY (parser, SQL deserializer):
EntityUri::block("abc123")        // produces block:abc123
EntityUri::from_raw("doc:uuid")   // parses scheme from string

// IN DOWNSTREAM CODE:
block.id.scheme()  // "block"
block.id.id()      // "abc123"   (bare ID, no scheme)
block.id.as_str()  // "block:abc123"
```

**Critical**: Source block IDs like `j-09-::src::0` contain `::` which is valid RFC 3986 scheme syntax. The parser MUST use `EntityUri::block()` explicitly, NOT `EntityUri::from_raw()`, to avoid misidentifying `j-09-` as the scheme.

**Org files store bare IDs** (no scheme). Parser adds schemes. Renderer strips them: writes `block.id.id()` not `block.id.as_str()`.

### Typed Domain Enums

From `crates/holon-api/src/types.rs` — instead of `String` for domain concepts:

```rust
pub enum ContentType {
    Headline, Text, Source, HolonPrql, Render, ...
}

pub enum TaskState {
    Todo, InProgress, Done, Cancelled, ...
}

pub enum SourceLanguage {
    Rust, Python, Sql, Prql, Rhai, Render, ...
}

pub enum QueryLanguage {
    Prql, Sql, Gql,
}

pub enum Region {
    LeftSidebar, MainPanel, RightSidebar,
}
```

This eliminates `match str.as_str() { "TODO" => ... }` scattered across files — those patterns indicate a string that should be an enum.

## Value Enum

`crates/holon-api/src/lib.rs` — the universal dynamic value type used across the entire system.

### Why an Enum, Not `serde_json::Value`?

- Flutter/Dart FFI compatibility (flutter_rust_bridge needs non-generic types)
- Type-safe accessors with proper `None` semantics
- DateTime and Json variants that distinguish opaque blobs from strings
- `ALLOW(ok)` annotations mark the few places `.ok()` is intentional (boundary parses)

### Important Coercions

`as_i64()` also parses `Value::String(s)` as integer — SQLite TEXT-affinity columns store integers as strings. This is documented via `// SQLite TEXT-affinity columns` comment.

### StorageEntity

```rust
pub type StorageEntity = HashMap<String, Value>;
```

The raw row type from Turso queries. All CDC events carry `StorageEntity`. Converted to typed structs via `TryFromEntity` at the boundary.

## TypeDefinition & FieldSchema

`crates/holon-api/src/entity.rs`:

```rust
pub struct TypeDefinition {
    pub name: String,
    pub fields: Vec<FieldSchema>,
    pub indexes: Vec<IndexDef>,
}

pub struct FieldSchema {
    pub name: String,
    pub sql_type: String,
    pub lifetime: FieldLifetime,
    pub nullable: bool,
}

pub enum FieldLifetime {
    Stored,           // persisted in DB column
    Computed(CompiledExpr),  // computed by Rhai at read time
    Derived,          // computed from other fields at read time
}
```

`FieldLifetime::Computed` takes a `CompiledExpr` (Rhai AST) evaluated per-row. Used for e.g. `task_weight` in Petri net materialization.

## DynamicEntity

`crates/holon-api/src/entity.rs` — a type-erased runtime entity representation. Used when the entity type isn't known at compile time.

```rust
pub struct DynamicEntity {
    pub type_name: String,
    pub fields: HashMap<String, Value>,
}
```

## IntoEntity / TryFromEntity

Traits for bidirectional conversion between typed structs and `StorageEntity`:
```rust
pub trait IntoEntity {
    fn into_entity(self) -> StorageEntity;
    fn type_definition() -> TypeDefinition;
}

pub trait TryFromEntity: Sized {
    fn try_from_entity(entity: StorageEntity) -> Result<Self>;
}
```

Derived by `#[derive(Entity)]` macro.

## Be Suspicious Of

- `match str.as_str() { ... }` scattered across files → should be an enum
- `.ok()` on parse results → silent data loss, use `.expect()` or `?`
- `_ => default` on parse results → same issue

## Related Pages

- [[entities/holon-api]] — full type library
- [[concepts/cdc-and-streaming]] — `StorageEntity`, `Change<T>`
- [[entities/holon-macros]] — `#[derive(Entity)]` generates `IntoEntity`/`TryFromEntity`
