//! @c4 component
//! @c4 layer Adapters
//! Pattern: Adapter
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//!
//! `holon-turso` — the Turso (SQLite-IVM) storage adapter.
//!
//! This crate owns everything specific to the Turso substrate: the
//! [`TursoBackend`](turso::TursoBackend) actor + `DbHandle`, the
//! materialized-view manager, the SQL parser/transform layer, the GQL
//! graph-schema registry, and the `SchemaModule` lifecycle contract. The
//! adapter-agnostic storage abstractions it builds on (`StorageBackend`,
//! `Filter`, `Resource`, `StorageError`) live in `holon-core`, so a wiring
//! without Turso need not depend on this crate at all (ADR 0004 Phase 9 — Turso
//! is one of four storage adapters).

pub mod block_table_names;
pub mod durable_state;
pub mod dynamic_schema_module;
pub mod engine_functions;
pub mod graph_schema;
pub mod matview_manager;
pub mod schema_module;
pub mod schema_modules;
pub mod sql_parser;
pub mod sql_utils;
pub mod turso;
pub mod turso_actor_stats;
pub mod util;
