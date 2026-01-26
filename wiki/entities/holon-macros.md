---
title: holon-macros crate (procedural macros)
type: entity
tags: [crate, macros, codegen, proc-macro]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon-macros/src/builder_registry.rs
  - crates/holon-macros/src/lib.rs
---

# holon-macros crate

Procedural macros for code generation. Reduces boilerplate across entity definitions, builder registration, and operation trait generation.

## #[Entity] derive macro

Derives `IntoEntity`, `TryFromEntity`, and related storage traits for a struct. Generates:
- `TypeDefinition` with `FieldSchema` per field
- SQL DDL for the entity table
- Serialization/deserialization code for `StorageEntity` (= `HashMap<String, Value>`)

Usage: `#[derive(Entity)]` on a struct. Fields become SQL columns with types inferred from Rust types. `FieldLifetime::Computed` fields get Rhai expressions evaluated at read time.

## #[operations_trait] macro

Generates an `OperationRegistry` impl and associated dispatch code for a trait. Used on operation traits like `BlockOperations`, `TaskOperations`. Produces `OperationDescriptor` entries for each method.

`#[affects(...)]` attribute on operation methods lists the fields that operation modifies — used for CDC-level field delta tracking and UI optimizations.

## builder_registry macro

`crates/holon-macros/src/builder_registry.rs` — build-time macro that generates the shadow builder dispatch table.

Works by:
1. Scanning `shadow_builders/` directory at **compile time** (`build.rs`)
2. Parsing each `pub fn render(...)` signature to extract parameter names
3. Generating a `match name { "text" => text::render(...), ... }` dispatch table

`RenderSignature` parses param names, handling both:
- Normal mode: destructure `ViewKind` fields as individual params
- Pass-through mode: first param is `node` → pass the full `ViewModel`

`snake_to_pascal()` converts builder file names to their `ViewKind` variant names.

## holon-macros-test

`crates/holon-macros-test/` — expansion tests for macro correctness. Tests that `#[Entity]` generates expected `TypeDefinition` and that `#[operations_trait]` produces correct `OperationDescriptor` entries.

## Related Pages

- [[entities/holon-api]] — `TypeDefinition`, `FieldSchema`, `FieldLifetime`
- [[entities/holon-frontend]] — builder_registry used in `RenderInterpreter`
