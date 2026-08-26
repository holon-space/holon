//! The DEGRADED ("shows source") SUT component: a real **no-Turso** block-query
//! frontend. It boots [`holon_app::from_block_query_source`] (via
//! [`holon_app::register_block_query_frontend`]) over a Loro
//! [`BlockQuerySource`] seeded with a query-source child block, so the
//! production `loro_ui_watcher::derive_render_expr` takes its `source_editor`
//! arm — the capability-driven degradation of ADR 0004 Phase 9 (no query engine
//! ⇒ offer only the bare `source` view mode).
//!
//! It provides [`SutRenderer`] (the root render kind is `source_editor`) but
//! deliberately does NOT provide `SutQueryResults` — there is no query engine
//! in this wiring. That absence is the negative-selection (`sut_absent`)
//! discriminator that selects the degraded
//! `inv-viewmodel-shows-source-when-no-query` twin and deselects the full-mode
//! `inv-viewmodel-decompiled-rows-match-query` twin. The component WRAPS the
//! real production render path (no re-implementation): the same `LoroUiWatcher`
//! → `ReactiveEngine::ensure_watching` → `ReactiveRenderedRows::snapshot`
//! surface the full-mode [`super::components::HeadlessFrontendComponent`]
//! reads, minus the Turso CDC query engine.

use std::sync::Arc;
use std::time::Duration;

use holon::di::build_no_turso_container;
use holon_api::BlockContent;
use holon_api::EntityUri;
use holon_api::repository::CoreOperations;
use holon_api::repository::Lifecycle;
use holon_app::register_block_query_frontend;
use holon_frontend::FrontendSession;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::BuilderServicesSlot;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::reactive::ReactiveRenderedRows;
use holon_loro::LoroBackend;
use holon_loro_wiring::loro_block_query_source::register_loro_block_query_source;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::WidgetSnapshot;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapProvider;

use crate::pbt::vm_snapshot::view_model_to_snapshot;

/// A composition component wrapping a real no-Turso block-query frontend. Owns
/// the DI injector, `FrontendSession`, `ReactiveEngine`, and the seeded
/// `LoroBackend` so the render watch and its background tasks stay alive for
/// the component's lifetime. `#[doc]` transient — scaffolding for the degraded
/// twin, folded into the ONE PBT and deleted as the composition converges.
#[doc(hidden)]
pub struct BlockQueryFrontendComponent {
    reactive: Arc<ReactiveEngine>,
    _session: Arc<FrontendSession>,
    _injector: fluxdi::Injector,
    _backend: Arc<LoroBackend>,
    /// The query-page block whose query-source child drives the `source_editor`
    /// render — the root this component reports a render kind for.
    root: EntityUri,
}

impl BlockQueryFrontendComponent {
    /// Boot a no-Turso block-query frontend over a Loro tree seeded with a
    /// single query page (a parent block plus one query-source child). The
    /// child's `source_language` is a query language, so
    /// `derive_render_expr` resolves the degraded `source_editor` render
    /// for the parent — VERIFIED by the minimal positive test (else the
    /// degraded twin would Skip forever with no teeth).
    pub async fn new() -> Self {
        let backend = LoroBackend::create_new("block-query-degraded".to_string())
            .await
            .expect("create LoroBackend for degraded frontend");
        let page = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::text("Query page"),
                None,
            )
            .await
            .expect("seed query page");
        // A query-source child: `source_language` parses to a real query language
        // (`holon_prql` → `QueryLanguage::HolonPrql`), so `as_query()` is `Some` and
        // `derive_render_expr` degrades the parent to `source_editor`. A bare alias
        // like `"prql"` would parse to `SourceLanguage::Other` and render a leaf.
        backend
            .create_block(
                page.id.clone(),
                BlockContent::source("holon_prql", "from blocks"),
                None,
            )
            .await
            .expect("seed query-source child");
        let backend = Arc::new(backend);

        let injector = build_no_turso_container(":memory:".into(), {
            let backend = backend.clone();
            move |inj| {
                register_loro_block_query_source(inj, backend.clone());
                register_block_query_frontend(inj);
                Ok(())
            }
        })
        .await
        .expect("assemble no-Turso block-query container");

        let session = injector.resolve::<FrontendSession>();
        let reactive = injector.resolve::<ReactiveEngine>();
        // Populate the OnceLock that breaks the engine↔interpreter cycle — the same
        // step the Turso path performs after resolving the engine.
        let slot = injector.resolve::<BuilderServicesSlot>();
        let services: Arc<dyn BuilderServices> = reactive.clone();
        slot.0.set(services).ok(); // ALLOW(ok): OnceLock set — idempotent

        Self {
            reactive,
            _session: session,
            _injector: (*injector).clone(),
            _backend: backend,
            root: page.id,
        }
    }

    /// Resolve a ready (non-loading) reactive watch for `uri`, polling the
    /// engine until its first results load. Mirrors
    /// [`super::components::HeadlessFrontendComponent::resolve_watch`].
    async fn resolve_watch(&self, uri: &EntityUri) -> Option<Arc<ReactiveRenderedRows>> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let rqr = self.reactive.ensure_watching(uri);
            if !rqr.is_loading() {
                return Some(rqr);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn services(&self) -> Arc<dyn BuilderServices> {
        self.reactive.clone()
    }
}

#[async_trait::async_trait(?Send)]
impl SutRenderer for BlockQueryFrontendComponent {
    async fn render_tree_of(&self, id: &EntityUri) -> Option<String> {
        let rqr = self.resolve_watch(id).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.services();
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        Some(vm.pretty_print(0))
    }

    async fn widget_tree_snapshot(&self) -> WidgetSnapshot {
        let empty = || WidgetSnapshot {
            kind: "empty".into(),
            entity_id: None,
            props: Default::default(),
            operations: Vec::new(),
            children: Vec::new(),
        };
        let Some(rqr) = self.resolve_watch(&self.root).await else {
            return empty();
        };
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.services();
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        view_model_to_snapshot(&vm)
    }

    /// No internal caching — plain forward to `widget_tree_snapshot`.
    async fn widget_tree_snapshot_fresh(&self) -> WidgetSnapshot {
        self.widget_tree_snapshot().await
    }

    async fn collection_row_ids(
        &self,
        block_id: &EntityUri,
    ) -> Option<std::collections::BTreeSet<EntityUri>> {
        self.reactive.registry_row_ids(block_id)
    }

    async fn root_data_row_ids(&self) -> std::collections::BTreeSet<EntityUri> {
        let Some(rqr) = self.resolve_watch(&self.root).await else {
            return Default::default();
        };
        let (_, data_rows) = rqr.snapshot();
        data_rows
            .iter()
            .filter_map(|r| {
                r.get("id")
                    .and_then(|v| v.as_string())
                    .and_then(|s| EntityUri::parse(s).ok())
            })
            .collect()
    }

    async fn widget_tree_for(&self, block_id: &EntityUri) -> Option<WidgetSnapshot> {
        let rqr = self.resolve_watch(block_id).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.services();
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        Some(view_model_to_snapshot(&vm))
    }

    async fn root_content_comparison(
        &self,
        visible_columns: &[String],
    ) -> Option<(Vec<String>, Vec<String>)> {
        let rqr = self.resolve_watch(&self.root).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.services();
        let display_tree =
            holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        let rendered_rows = crate::display_assertions::extract_rendered_rows(&display_tree);
        if rendered_rows.is_empty() || visible_columns.is_empty() || data_rows.is_empty() {
            return None;
        }
        let data_content: Vec<String> = data_rows
            .iter()
            .filter_map(|r| {
                r.get("content")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
            })
            .collect();
        let rendered_content: Vec<String> = rendered_rows
            .iter()
            .filter_map(|r| {
                r.get("content")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
            })
            .collect();
        Some((rendered_content, data_content))
    }

    async fn root_render_ready(&self) -> bool {
        let Some(rqr) = self.resolve_watch(&self.root).await else {
            return false;
        };
        let (render_expr, data_rows) = rqr.snapshot();
        let placeholder = matches!(
            &render_expr,
            holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading" || name == "spacer"
        );
        if placeholder {
            return false;
        }
        let services = self.services();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        }))
        .is_ok()
    }

    async fn root_render_kind(&self) -> Option<String> {
        let rqr = self.resolve_watch(&self.root).await?;
        match rqr.snapshot().0 {
            holon_api::RenderExpr::FunctionCall { name, .. }
                if name != "loading" && name != "spacer" =>
            {
                Some(name)
            }
            _ => None,
        }
    }
}

impl CapProvider for BlockQueryFrontendComponent {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        // Renderer ONLY — no `SutQueryResults` (no query engine). That absence is
        // exactly what selects the degraded twin and deselects the full-mode one.
        caps.insert(self as Arc<dyn SutRenderer>);
    }
}
