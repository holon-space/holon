//! Query compilation context — determines what virtual tables resolve to.
//!
//! Relocated from `holon::api::backend_engine` (storage de-leak Stage 1):
//! the context is pure data (entity ids + a path prefix), so it belongs in
//! the storage-agnostic API layer.

use crate::entity_uri::EntityUri;

/// Context for query compilation - determines what virtual tables resolve to
#[derive(Debug, Clone)]
pub struct QueryContext {
    /// Current block ID for `from children` resolution. None = root level
    /// (parent_id IS NULL)
    pub current_block_id: Option<EntityUri>,
    /// Parent of current block for `from siblings` resolution
    pub context_parent_id: Option<EntityUri>,
    /// Path prefix for descendants queries (e.g., "/block-123/%")
    /// Computed from block_with_path matview when context is created with path
    /// lookup. This is a SQL LIKE prefix, not an entity ID.
    pub context_path_prefix: Option<String>,
}

impl QueryContext {
    /// Create a root-level context (for queries at the top level)
    pub fn root() -> Self {
        Self {
            current_block_id: None,
            context_parent_id: None,
            context_path_prefix: None,
        }
    }

    /// Create a context for a specific block
    pub fn for_block(block_id: &EntityUri, parent_id: Option<EntityUri>) -> Self {
        Self {
            current_block_id: Some(block_id.clone()),
            context_parent_id: parent_id,
            context_path_prefix: None,
        }
    }

    /// Create a context for a specific block with path prefix for descendants
    /// queries
    pub fn for_block_with_path(
        block_id: &EntityUri,
        parent_id: Option<EntityUri>,
        path: String,
    ) -> Self {
        Self {
            current_block_id: Some(block_id.clone()),
            context_parent_id: parent_id,
            context_path_prefix: Some(format!("{}/", path)),
        }
    }
}
