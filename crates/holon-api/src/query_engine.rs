//! The query-execution capability (ADR 0004 — "Turso is one of four").
//!
//! `QueryEngine` is the seam the frontend's query path depends on instead of a
//! concrete storage backend. Compiling a query and executing/​watching it
//! against materialised views is a capability that **only** the Turso wiring
//! provides; a no-Turso (Loro-only) session has no `QueryEngine`, so the
//! frontend offers query blocks the `source` view mode only. Holding this as
//! `Option<Arc<dyn QueryEngine>>` makes the absence a representable, typed
//! fact rather than a panic waiting to happen behind `engine()`.
//!
//! This is the storage-agnostic core: every signature speaks holon-api types.
//! The raw-SQL surface (`compile_to_sql`, `query_and_watch`, `execute_query`)
//! lives on `holon::api::SqlQueryEngine`, a Turso-private extension trait for
//! MCP debug tools, tests, and holon-internal code (storage de-leak Stage 10).

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

use crate::query_context::QueryContext;
use crate::{EnrichedChangeStream, EntityUri, LinkCandidate, QueryLanguage, Value};

/// Compile + execute + watch queries, behind storage-agnostic types.
/// Implemented by the Turso `BackendEngine`; absent in a no-Turso wiring.
#[async_trait]
pub trait QueryEngine: Send + Sync {
    /// Resolve a block's hierarchical path from the `blocks_with_paths`
    /// materialised view (used as a LIKE prefix for descendants queries).
    /// Matview-backed, so it lives on the query capability.
    async fn lookup_block_path(&self, block_id: &EntityUri) -> Result<String>;

    /// Compile a query (PRQL/GQL/SQL), set up CDC streaming, and return the
    /// **enriched** change stream. SQL compilation and enrichment both happen
    /// behind this capability — the storage-agnostic layers never see SQL
    /// strings or the raw Turso stream (storage de-leak Stage 2).
    async fn watch_query(
        &self,
        query: &str,
        language: QueryLanguage,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<EnrichedChangeStream>;

    /// Search blocks/pages matching `filter` for the `[[` link-autocomplete
    /// popup. Replaces the raw-SQL `popup_query` capability: the search SQL
    /// lives behind the impl, the frontend only sees typed candidates.
    async fn search_link_candidates(&self, filter: &str) -> Result<Vec<LinkCandidate>>;

    /// Non-settling read of a single block's `content` straight from the
    /// write table (`block_raw`). Used by the headless editor mirror, which
    /// must see exactly what a production editor's SQL read would see —
    /// **without** awaiting CDC quiescence (`BlockQuerySource::snapshot`
    /// settles, which would mask the projection races the PBTs hunt).
    /// `None` when the row hasn't materialised yet.
    async fn block_content_by_id(&self, id: &EntityUri) -> Result<Option<String>>;
}
