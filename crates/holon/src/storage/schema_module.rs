//! Schema module trait for declarative database schema lifecycle.
//!
//! Each schema component implements `SchemaModule`, declaring:
//! - `provides()`: Resources this module creates (tables, views)
//! - `requires()`: Resources this module depends on
//! - `ensure_schema()`: The actual DDL execution
//!
//! Dependency ordering is handled by FluxDI's `resolve_all_eager()` via
//! phantom-typed `DbReady<R>` markers in `di::schema_providers`. The
//! `SchemaModule` trait is retained purely for encapsulating DDL execution
//! and resource metadata.

use async_trait::async_trait;

use holon_api::entity::{GraphEdgeDef, GraphNodeDef};

use super::resource::Resource;
use super::turso::DbHandle;
use super::types::Result;

/// Descriptor for a multi-valued edge-typed field that projects to a junction
/// table (per Track 1A / H5).
///
/// At write time, `SqlOperationProvider` recognises a param whose key matches
/// `field` and routes the `Value::Array` payload through DELETE+INSERT against
/// `join_table` instead of folding it into the `properties` JSON blob. At
/// query time, `graph_schema::build()` wires a `JoinTableEdgeResolver` so GQL
/// `MATCH (a)-[:edge]->(b)` patterns dispatch to a JOIN against the same
/// junction.
#[derive(Debug, Clone)]
pub struct EdgeFieldDescriptor {
    /// Owning entity type name (e.g. "block") — matches the entity the
    /// field appears on.
    pub entity: String,
    /// Field name as it appears in `Block.properties` / params (e.g.
    /// "blocked_by", "tags").
    pub field: String,
    /// Junction table name (e.g. "task_blockers", "block_tags").
    pub join_table: String,
    /// Column on `join_table` holding the source entity's id.
    pub source_col: String,
    /// Column on `join_table` holding the target entity's id (or, for tag-style
    /// edges, the literal tag value).
    pub target_col: String,
}

/// A module that manages a set of database schema objects.
///
/// Implement this trait for each logical group of database objects that
/// should be created together. Dependency ordering between modules is
/// handled by FluxDI's `DbReady<R>` providers in `di::schema_providers`.
#[async_trait]
pub trait SchemaModule: Send + Sync {
    /// Unique name for this module (used in logging and error messages).
    fn name(&self) -> &str;

    /// Resources this module creates (tables, views, materialized views).
    ///
    /// These resources will be automatically registered with the `DbHandle`
    /// after `ensure_schema()` completes successfully.
    fn provides(&self) -> Vec<Resource>;

    /// Resources this module depends on.
    ///
    /// Used by `DynamicSchemaModule` to verify prerequisites at runtime.
    /// For core schema modules, ordering is enforced at the DI level.
    fn requires(&self) -> Vec<Resource>;

    /// Execute DDL to create/update the schema objects.
    ///
    /// This method should be idempotent (safe to call multiple times).
    /// Use `CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`, etc.
    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()>;

    /// Optional post-schema initialization (e.g., inserting default data).
    ///
    /// Called after `ensure_schema()` succeeds but before resources are
    /// marked as available.
    async fn initialize_data(&self, _db_handle: &DbHandle) -> Result<()> {
        Ok(())
    }

    /// Optional GQL graph schema contributions from this module.
    ///
    /// Override to register graph nodes (for views/matviews) and edges
    /// that aren't derivable from Entity `#[reference]` annotations.
    fn graph_contributions(&self) -> (Vec<GraphNodeDef>, Vec<GraphEdgeDef>) {
        (vec![], vec![])
    }

    /// Multi-valued edge-typed fields that project to a junction table.
    ///
    /// Returned descriptors are consumed by both `SqlOperationProvider`
    /// (for DELETE+INSERT routing on writes) and `graph_schema::build()`
    /// (for `JoinTableEdgeResolver` wiring on reads). Default is empty —
    /// only modules that own entities with edge-typed fields override this.
    fn edge_fields(&self) -> Vec<EdgeFieldDescriptor> {
        Vec::new()
    }
}
