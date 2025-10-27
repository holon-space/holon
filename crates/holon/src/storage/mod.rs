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
pub mod turso_block_link_indexer;
pub mod turso_sink_reader;

#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers;

pub use backend::*;
pub use block_table_names::{BLOCK_READ_TABLE, BLOCK_WRITE_TABLE};
pub use holon_core::fractional_index::*;
pub use resource::Resource;
pub use schema_module::{EdgeFieldDescriptor, SchemaModule};
pub use sql_parser::{
    ChangeOriginInjector, JsonAggregationSqlTransformer, SqlTransformer, apply_sql_transforms,
    extract_created_tables, extract_table_refs, inject_entity_name, inject_entity_name_into_sql,
    parse_sql, sql_to_string,
};
pub use sync_token_store::*;
pub use turso::{DatabasePhase, DbCommand, DbHandle, priority};
pub use turso_block_link_indexer::TursoBlockLinkIndexer;
pub use turso_sink_reader::TursoSinkReader;
