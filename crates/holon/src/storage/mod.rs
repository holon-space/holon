pub mod backend;
pub mod block_table_names;
pub mod dynamic_schema_module;
pub mod graph_schema;
pub mod resource;
pub mod schema_module;
pub mod schema_modules;
pub mod sql_parser;
pub mod sql_utils;
pub mod sync_token_store;
pub mod turso;
pub mod turso_actor_stats;
pub mod types;

#[cfg(test)]
pub mod test_helpers;

#[cfg(test)]
mod turso_repro_test;

#[cfg(test)]
mod turso_ivm_cdc_zero_changes_repro;

#[cfg(test)]
mod turso_ivm_union_all_insert_repro;

#[cfg(test)]
mod turso_ivm_navigation_cursor_repro;

#[cfg(test)]
mod turso_ivm_split_block_cdc_drop_repro;

#[cfg(test)]
mod turso_matview_first_open_test;

#[cfg(test)]
mod cdc_base_vs_matview_repro;

/// Split a semicolon-delimited SQL file into individual statements.
///
/// Skips `;` inside `--` line comments — otherwise comments like
/// `-- closed (omitted); retained for back/forward` truncate the
/// surrounding CREATE TABLE statement and the parser sees "incomplete input".
pub fn sql_statements(content: &str) -> impl Iterator<Item = &str> {
    let bytes = content.as_bytes();
    let mut splits: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let mut in_line_comment = false;
    let mut prev_dash = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_line_comment {
            if b == b'\n' {
                in_line_comment = false;
            }
            prev_dash = false;
            continue;
        }
        if b == b'-' && prev_dash {
            in_line_comment = true;
            prev_dash = false;
            continue;
        }
        prev_dash = b == b'-';
        if b == b';' {
            splits.push((start, i));
            start = i + 1;
        }
    }
    splits.push((start, bytes.len()));
    splits
        .into_iter()
        .map(move |(a, b)| content[a..b].trim())
        .filter(|s| !s.is_empty())
}

pub use backend::*;
pub use block_table_names::{BLOCK_READ_TABLE, BLOCK_WRITE_TABLE};
pub use holon_core::fractional_index::*;
pub use resource::Resource;
pub use schema_module::{EdgeFieldDescriptor, SchemaModule};
pub use schema_modules::{
    BlockHierarchySchemaModule, BlockSchemaModule, CoreSchemaModule, IdentitySchemaModule,
    LinkSchemaModule, NavigationSchemaModule, OperationsSchemaModule, SyncStateSchemaModule,
};
pub use sql_parser::{
    ChangeOriginInjector, JsonAggregationSqlTransformer, SqlTransformer, apply_sql_transforms,
    extract_created_tables, extract_table_refs, inject_entity_name, inject_entity_name_into_sql,
    parse_sql, sql_to_string,
};
pub use sync_token_store::*;
pub use turso::{DatabasePhase, DbCommand, DbHandle, priority};
pub use types::*;
