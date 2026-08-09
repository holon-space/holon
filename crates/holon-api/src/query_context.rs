//! Query compilation context — determines what virtual tables resolve to.
//!
//! Relocated from `holon::api::backend_engine` (storage de-leak Stage 1):
//! the context is pure data (entity ids + a path context), so it belongs in
//! the storage-agnostic API layer.

use crate::entity_uri::EntityUri;

/// How a query's `from descendants` path predicate is bound.
///
/// This splits the former `Option<String>` prefix so the ambiguous `None` —
/// which once conflated "no path filter wanted" with "path could not be
/// resolved" — cannot exist. The unresolvable case never becomes a
/// `PathContext` at all: resolution returns `Err` (surfaced as a visible
/// degraded banner) before a `QueryContext` is built. The old `__NO_PATH__/`
/// sentinel, which silently matched zero rows, is therefore unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PathContext {
    /// No path predicate — `from descendants` returns every row
    /// (`text.starts_with` against an empty prefix matches all paths).
    Unfiltered,
    /// Restrict `from descendants` to blocks whose path starts with this
    /// already-trailing-slash-terminated prefix.
    Under(String),
}

impl PathContext {
    /// The SQL LIKE prefix this context binds to `$context_path_prefix`.
    /// `Unfiltered` binds an empty string, so `text.starts_with` matches every
    /// row rather than the sentinel's zero rows.
    pub fn prefix_literal(&self) -> &str {
        match self {
            PathContext::Unfiltered => "",
            PathContext::Under(prefix) => prefix,
        }
    }
}

/// Context for query compilation - determines what virtual tables resolve to
#[derive(Debug, Clone)]
pub struct QueryContext {
    /// Current block ID for `from children` resolution. None = root level
    /// (parent_id IS NULL)
    pub current_block_id: Option<EntityUri>,
    /// Parent of current block for `from siblings` resolution
    pub context_parent_id: Option<EntityUri>,
    /// Path predicate for `from descendants`. `Unfiltered` for a no-filter
    /// context; `Under(prefix)` once a block's path is resolved. An
    /// unresolvable path is an `Err` at resolution time, never a variant here.
    pub path_context: PathContext,
}

impl QueryContext {
    /// Create a root-level context (for queries at the top level).
    /// Descendants are unfiltered — a top-level `from descendants` spans every
    /// block.
    pub fn root() -> Self {
        Self {
            current_block_id: None,
            context_parent_id: None,
            path_context: PathContext::Unfiltered,
        }
    }

    /// Create a context for a specific block WITHOUT a resolved descendants
    /// path. Suitable for `from children` / `from siblings`, which key off the
    /// block/parent ids and never consult the path. `from descendants` under
    /// such a context is unfiltered — use [`Self::for_block_with_path`] to
    /// scope descendants to the block's subtree.
    pub fn for_block(block_id: &EntityUri, parent_id: Option<EntityUri>) -> Self {
        Self {
            current_block_id: Some(block_id.clone()),
            context_parent_id: parent_id,
            path_context: PathContext::Unfiltered,
        }
    }

    /// Create a context for a specific block with a resolved path prefix for
    /// descendants queries.
    pub fn for_block_with_path(
        block_id: &EntityUri,
        parent_id: Option<EntityUri>,
        path: String,
    ) -> Self {
        Self {
            current_block_id: Some(block_id.clone()),
            context_parent_id: parent_id,
            path_context: PathContext::Under(format!("{}/", path)),
        }
    }
}
