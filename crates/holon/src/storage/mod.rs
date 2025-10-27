pub mod backend;
pub mod graph_schema;
pub mod resource;
pub mod schema;
pub mod schema_module;
pub mod schema_modules;
pub mod sql_parser;
pub mod sql_utils;
pub mod sync_token_store;
pub mod turso;
pub mod types;

#[cfg(test)]
pub mod test_helpers;

#[cfg(test)]
mod turso_repro_test;

#[cfg(test)]
mod turso_ivm_cdc_zero_changes_repro;

#[cfg(test)]
mod turso_ivm_union_all_insert_repro;

/// Split a semicolon-delimited SQL file into individual statements.
pub fn sql_statements(content: &str) -> impl Iterator<Item = &str> {
    content.split(';').map(str::trim).filter(|s| !s.is_empty())
}

pub use backend::*;
pub use holon_core::fractional_index::*;
pub use resource::Resource;
pub use schema::*;
pub use schema_module::{SchemaModule, SchemaRegistry, SchemaRegistryError};
pub use schema_modules::{
    BlockHierarchySchemaModule, CoreSchemaModule, NavigationSchemaModule, OperationsSchemaModule,
    SyncStateSchemaModule, create_core_schema_registry,
};
pub use sql_parser::{
    ChangeOriginInjector, JsonAggregationSqlTransformer, SqlTransformer, apply_sql_transforms,
    extract_created_tables, extract_table_refs, inject_entity_name, inject_entity_name_into_sql,
    parse_sql, sql_to_string,
};
pub use sync_token_store::*;
pub use turso::{DatabasePhase, DbCommand, DbHandle, priority};
pub use types::*;
