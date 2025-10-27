//! `BuilderServices` over a bare Turso `BackendEngine` — the MCP
//! `describe_ui` path (storage de-leak Stage 10: lives in the wiring crate
//! because it names the concrete engine; holon-frontend stays
//! storage-agnostic).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use futures_signals::signal::Mutable;
use holon::api::BackendEngine;
use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::entity_profile::ProfileCache;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::QueryContext;
use holon_frontend::RenderContext;
use holon_frontend::WidgetState;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::table_expr;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::render_interpreter::RenderInterpreter;

/// `BuilderServices` stub for the MCP `describe_ui` path, which renders the
/// UI tree without an interactive `ReactiveEngine` (no focus, no provider
/// cache, no dispatch). This is NOT a test-fidelity concern: the E2E PBTs
/// do not use this stub — they run windowless but drive the real
/// `ReactiveEngine` (see the `BuilderServices` trait doc).
pub struct HeadlessBuilderServices {
    engine: Arc<BackendEngine>,
    interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
    rt_handle: tokio::runtime::Handle,
}

impl HeadlessBuilderServices {
    /// Construct from the current tokio runtime context. Panics loudly if
    /// called outside a tokio runtime — all real call sites already are.
    pub fn new(engine: Arc<BackendEngine>) -> Self {
        Self::with_handle(engine, tokio::runtime::Handle::current())
    }

    pub fn with_handle(engine: Arc<BackendEngine>, rt_handle: tokio::runtime::Handle) -> Self {
        Self {
            engine,
            interpreter: Arc::new(holon_frontend::shadow_builders::build_shadow_interpreter()),
            rt_handle,
        }
    }
}

impl BuilderServices for HeadlessBuilderServices {
    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel {
        self.interpreter.interpret(expr, ctx, self)
    }

    fn get_block_data(&self, _: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        (table_expr(), vec![])
    }

    fn resolve_profile(&self, row: &DataRow) -> Option<holon_api::RenderProfile> {
        let (profile, _computed) = self.engine.profile_resolver().resolve_with_variants(row);
        Some(profile.as_ref().clone())
    }

    fn profile_signal(&self) -> Mutable<Arc<ProfileCache>> {
        self.engine.profile_resolver().profile_signal()
    }

    fn watch_query(
        &self,
        _: &str,
        _: QueryLanguage,
        _: Option<QueryContext>,
    ) -> Result<holon_api::EnrichedChangeStream> {
        anyhow::bail!("HeadlessBuilderServices does not support live queries")
    }

    fn widget_state(&self, _: &str) -> WidgetState {
        WidgetState::default()
    }

    fn set_widget_open(&self, _: &str, _: bool) {
        // Headless services have no UI to toggle.
    }

    fn dispatch_intent(&self, intent: holon_frontend::operations::OperationIntent) {
        tracing::warn!(
            "HeadlessBuilderServices.dispatch_intent({}.{}) — no-op in headless mode",
            intent.entity_name,
            intent.op_name
        );
    }

    fn present_op(
        &self,
        op: holon_api::render_types::OperationDescriptor,
        _: HashMap<String, holon_api::Value>,
    ) {
        panic!(
            "HeadlessBuilderServices::present_op({}.{}) — op_button must not be reached under a \
             non-interactive services instance. Its YAML branch is gated by `if_space(<600)` in \
             an interactive session; reaching this panic means the render path wired an \
             interactive builder into a headless one — fix the render path, do not swallow this.",
            op.entity_name, op.name
        );
    }

    fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.rt_handle.clone()
    }

    fn search_link_candidates(
        &self,
        filter: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<holon_api::LinkCandidate>>>
                + Send
                + 'static,
        >,
    > {
        use holon_api::QueryEngine as _;
        let engine = self.engine.clone();
        let filter = filter.to_string();
        Box::pin(async move { engine.search_link_candidates(&filter).await })
    }
}
