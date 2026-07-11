// Shared storage abstractions now live in `holon-core` (re-export shims).
pub mod backend;
pub mod resource;

// The Turso adapter now lives in the `holon-turso` crate. These re-exports
// keep the historical `crate::storage::*` paths resolving while making the
// dependency on `holon-turso` explicit (ADR 0004 Phase 9).
pub use holon_turso::block_table_names;
pub use holon_turso::dynamic_schema_module;
pub use holon_turso::graph_schema;
pub use holon_turso::schema_module;
pub use holon_turso::sql_parser;
pub use holon_turso::sql_utils;
pub use holon_turso::turso;
pub use holon_turso::turso_actor_stats;

pub mod sync_token_store;
pub mod turso_sink_reader;

#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers;

pub use backend::*;
pub use block_table_names::BLOCK_READ_TABLE;
pub use block_table_names::BLOCK_WRITE_TABLE;
pub use holon_core::fractional_index::*;
pub use resource::Resource;
pub use schema_module::EdgeFieldDescriptor;
pub use schema_module::SchemaModule;
pub use sql_parser::ChangeOriginInjector;
pub use sql_parser::JsonAggregationSqlTransformer;
pub use sql_parser::SqlTransformer;
pub use sql_parser::apply_sql_transforms;
pub use sql_parser::extract_created_tables;
pub use sql_parser::extract_table_refs;
pub use sql_parser::inject_entity_name;
pub use sql_parser::inject_entity_name_into_sql;
pub use sql_parser::parse_sql;
pub use sql_parser::sql_to_string;
pub use sync_token_store::*;
pub use turso::DatabasePhase;
pub use turso::DbCommand;
pub use turso::DbHandle;
pub use turso::priority;
pub use turso_sink_reader::TursoSinkReader;
