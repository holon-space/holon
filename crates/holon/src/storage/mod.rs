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

// Concrete schema DDL (owns the bundled `sql/` files) stays in `holon`.
pub mod schema_modules;
pub mod sync_token_store;
pub mod turso_block_link_indexer;
pub mod turso_sink_reader;

#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers;

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

/// Load type definitions from YAML files in a directory.
/// Registers each type in `registry` and creates extension tables via DynamicSchemaModule.
///
/// Lives here (not on `TypeRegistry` in `holon-profiles`) because it drives
/// Turso-side schema DDL through `DbHandle`.
pub async fn load_types_from_directory(
    registry: &holon_profiles::TypeRegistry,
    dir: &std::path::Path,
    db_handle: &DbHandle,
) -> anyhow::Result<Vec<String>> {
    use anyhow::Context;
    use dynamic_schema_module::DynamicSchemaModule;
    use holon_api::TypeDefinition;

    let mut loaded = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!("Types directory {:?} not found, skipping", dir);
            return Ok(loaded);
        }
        Err(e) => {
            return Err(anyhow::anyhow!(
                "Failed to read types directory {:?}: {e}",
                dir
            ));
        }
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read type definition {:?}", path))?;
        let type_def: TypeDefinition = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse type definition {:?}", path))?;

        let name = type_def.name.clone();

        if registry.contains(&name) {
            tracing::debug!("Type '{}' already registered, skipping YAML load", name);
            continue;
        }

        registry
            .register(type_def.clone())
            .with_context(|| format!("Failed to register type '{}' from {:?}", name, path))?;

        // Create extension table if it has fields
        if !type_def.fields.is_empty() {
            let module = DynamicSchemaModule::new(type_def);
            module
                .ensure_schema(db_handle)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create table for type '{}': {e}", name))?;
            db_handle
                .mark_available(module.provides())
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to mark resources available for type '{}': {e}",
                        name
                    )
                })?;
        }

        tracing::info!("Loaded type definition '{}' from {:?}", name, path);
        loaded.push(name);
    }

    Ok(loaded)
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
pub use turso_block_link_indexer::TursoBlockLinkIndexer;
pub use turso_sink_reader::TursoSinkReader;
