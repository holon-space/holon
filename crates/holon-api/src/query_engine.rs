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

use crate::EnrichedChangeStream;
use crate::EntityUri;
use crate::LinkCandidate;
use crate::QueryLanguage;
use crate::Value;
use crate::query_context::QueryContext;

/// Everything an editor mount reads off the block's row to build its editable
/// surface.
///
/// A struct rather than a tuple because the surface is the ORG SOURCE the pair
/// `(content, marks)` reconstructs, not the content column: seeding the
/// stripped label shows markup-free text that is nonetheless styled, and hands
/// every caret offset to a column that does not carry those bytes
/// (`2026-08-18-editor-seeded-from-stripped-content-not-source`). Three
/// positional `Option<String>`s would have made that mistake easy to repeat.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditorSource {
    /// The stored content column — the stripped label, marks removed.
    pub content: Option<String>,
    /// The mark spans over `content`, in scalar offsets.
    pub marks: Vec<crate::MarkSpan>,
    pub task_state: Option<String>,
}

/// One open tab in a region: an open `navigation_history` row.
///
/// A tab with no `block_id` is BLANK — it names no page, so the region's panel
/// renders its default view. That is a tab in its own right, which is why this
/// is not shaped after `focus_roots` (that matview drops NULL-block rows).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenTab {
    /// Row identity — what `navigation.activate` and `navigation.close` target.
    pub history_id: i64,
    pub block_id: Option<EntityUri>,
    /// The page's first content line. `None` for a blank tab.
    pub caption: Option<String>,
}

/// A region's tab bar: every open tab in stable insertion order, plus the one
/// the region's cursor points at.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RegionTabs {
    pub tabs: Vec<OpenTab>,
    /// `history_id` of the active tab; `None` when the region has no cursor.
    pub active_history_id: Option<i64>,
}

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
    ///
    /// `renderer` is the requirement manifest of the renderer this
    /// subscription feeds. It travels with the subscription because the
    /// contract is per-binding: the same SQL can serve several renderers, and
    /// only the binding knows which columns the render is wrong without.
    async fn watch_query(
        &self,
        query: &str,
        language: QueryLanguage,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
        renderer: crate::render_requirements::RenderRequirements,
    ) -> Result<EnrichedChangeStream>;

    /// Sort-key spec (`col` for ascending, `-col` for descending) implied by
    /// the query's trailing `ORDER BY`, for the collection that renders its
    /// rows to sort by.
    ///
    /// Lives on the engine because compilation does: the frontend only ever
    /// holds the source query, and the clause is stripped from the matview
    /// body before it reaches storage.
    ///
    /// Default `None` — an engine with no SQL compiler declares no order, and
    /// its collections keep their own row order.
    // ALLOW(unused_param): trait default; overriding impls bind both
    fn ordering_spec(&self, _: &str, _: QueryLanguage) -> Result<Option<String>> {
        Ok(None)
    }

    /// Search blocks/pages matching `filter` for the `[[` link-autocomplete
    /// popup. Replaces the raw-SQL `popup_query` capability: the search SQL
    /// lives behind the impl, the frontend only sees typed candidates.
    async fn search_link_candidates(&self, filter: &str) -> Result<Vec<LinkCandidate>>;

    /// User-facing quick-open / content search (`cmd-K` modal). Returns two
    /// sections — `pages` (jump-to-page targets, `Page`-tagged) first, then
    /// `content` (full-text block matches) — so the modal renders them
    /// separately without re-deriving which rows are pages. The `Page`-tag
    /// join and the `LIKE` search live behind this capability, mirroring
    /// [`Self::search_link_candidates`].
    ///
    /// Default impl fails loud (returns `Err`) rather than faking an empty
    /// result — a `QueryEngine` without quick-open wiring must surface that.
    async fn quick_open_search(&self, filter: &str) -> Result<crate::QuickOpenResults> {
        let _ = filter;
        anyhow::bail!("QueryEngine::quick_open_search not implemented by this impl")
    }

    /// Resolve the page-ancestor breadcrumb trail for `block_id`: the chain of
    /// `Page`-tagged ancestors from root to (and including) the block's nearest
    /// page, in root→current order. Each entry is `(id, title)` where the title
    /// is the page's first content line. Reuses the same ancestor path the
    /// `block_with_path` matview maintains — no separate tree walk.
    ///
    /// Fails loud (default impl bails) rather than returning an empty trail: a
    /// breadcrumb that can't be resolved must surface, not silently vanish.
    async fn breadcrumb_trail(&self, block_id: &EntityUri) -> Result<Vec<LinkCandidate>> {
        let _ = block_id;
        anyhow::bail!("QueryEngine::breadcrumb_trail not implemented by this impl")
    }

    /// The block a region's panel is currently open on: the `focus_roots` row
    /// the region's `navigation_cursor` points at. `None` when the region has
    /// no open view.
    async fn region_view_root(&self, region: crate::Region) -> Result<Option<EntityUri>> {
        let _ = region;
        anyhow::bail!("QueryEngine::region_view_root not implemented by this impl")
    }

    /// Every open tab in `region` (insertion order) plus its active tab — what
    /// the chrome's tab count and tab list are a view of.
    ///
    /// Blank tabs are included; see [`OpenTab`].
    async fn region_open_tabs(&self, region: crate::Region) -> Result<RegionTabs> {
        let _ = region;
        anyhow::bail!("QueryEngine::region_open_tabs not implemented by this impl")
    }

    /// Non-settling read of a single block's `content` straight from the
    /// write table (`block_raw`). Used by the headless editor mirror, which
    /// must see exactly what a production editor's SQL read would see —
    /// **without** awaiting CDC quiescence (`BlockQuerySource::snapshot`
    /// settles, which would mask the projection races the PBTs hunt).
    /// `None` when the row hasn't materialised yet.
    async fn block_content_by_id(&self, id: &EntityUri) -> Result<Option<String>>;

    /// Non-settling read of a single block's stored task keyword
    /// (`properties.task_state`), the companion of
    /// [`Self::block_content_by_id`]. An editor needs it to run the live
    /// keyword-promotion guard the engine runs — a block that is already a task
    /// must not be proposed for promotion. `None` for a plain block or a row
    /// that hasn't materialised.
    async fn block_task_state_by_id(&self, id: &EntityUri) -> Result<Option<String>>;

    /// The columns the editable surface is projected from, in ONE read.
    ///
    /// One read rather than three because every editor mount needs all of
    /// them, and the keystone budgets count reads per action — extra round
    /// trips per focus would be a measurable regression for facts the first
    /// row already carried.
    async fn block_editor_source_by_id(&self, id: &EntityUri) -> Result<EditorSource> {
        let _ = id;
        anyhow::bail!("QueryEngine::block_editor_source_by_id not implemented by this impl")
    }

    /// The `#+TODO:` keywords declared by the document that owns `id` — its
    /// nearest `Page`-tagged ancestor, the block itself included. `None` when
    /// the document declares none, which is the caller's cue to apply the
    /// parser's defaults (the same precedence the org parser applies).
    ///
    /// The editable surface needs it: what a block's stored `task_state`
    /// PROJECTS to as vault syntax depends on whether this document declares
    /// that keyword at all, and an editor seeded with a keyword the document
    /// would read back as prose silently demotes the task on commit. Read once
    /// per focus, never per keystroke.
    ///
    /// Default impl fails loud rather than answering "no keywords": a wiring
    /// that cannot resolve the vocabulary must surface, because the quiet
    /// answer is indistinguishable from a document that declares none.
    async fn block_todo_keywords(&self, id: &EntityUri) -> Result<Option<Vec<crate::TaskState>>> {
        let _ = id;
        anyhow::bail!("QueryEngine::block_todo_keywords not implemented by this impl")
    }

    /// ONE-SHOT, non-watching read: compile + execute `query` exactly once and
    /// return its current rows. Unlike [`Self::watch_query`], this sets up
    /// **no** materialized view and **no** CDC stream.
    ///
    /// This is the ONLY sanctioned execution path for the advice weave's
    /// canonical read (anchor anti-join + `ORDER BY` + `LIMIT`, ADR 0022): that
    /// shape MUST NOT be handed to `watch_query`, which matview-izes any SQL
    /// and whose Turso IVM cannot maintain an anti-join FUSED WITH an
    /// aggregate/GROUP BY (see holon-advice `probe_ivm_shape_findings`).
    /// NOTE: a PLAIN anti-join in a non-aggregating outer view IS
    /// incrementally maintained (proven by
    /// `probe_outer_antijoin_is_incrementally_maintained`), so watching such a
    /// read is fine — the advice weaver does exactly that.
    /// See `holon_frontend::advice_weaver`.
    ///
    /// Default impl fails loud (returns `Err`) rather than silently returning
    /// an empty set — a `QueryEngine` that has not wired one-shot execution
    /// should surface that, not fake success. The Turso `BackendEngine`
    /// overrides it.
    async fn execute_query(
        &self,
        query: &str,
        language: QueryLanguage,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<Vec<crate::widget_spec::DataRow>> {
        let _ = (query, language, params, context);
        anyhow::bail!("QueryEngine::execute_query (one-shot) not implemented by this impl")
    }
}
