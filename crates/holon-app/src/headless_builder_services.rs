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

/// `BuilderServices` for non-interactive rendering — the MCP `describe_ui`
/// path and the composed PBT frontend slice both build their trees through it,
/// without an interactive `ReactiveEngine` (no focus, no provider cache, no
/// dispatch).
///
/// It IS a test-fidelity surface. The composed frontend slice's `services()`
/// constructs it (`frontend_slice/components.rs`), so every `SutRenderer` tree
/// the PBTs judge is interpreted here. What this impl cannot do, the PBTs
/// cannot see — which is how an unconditionally-failing `watch_query` painted
/// the default sidebar as an error widget in every headless run, unnoticed
/// until the error-widget oracle learned to walk per-block trees
/// (`2026-08-26-headless-services-render-live-query-blocks-as-error-widgets`).
pub struct HeadlessBuilderServices {
    engine: Arc<BackendEngine>,
    interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
    rt_handle: tokio::runtime::Handle,
    /// The `describe_ui` path holds a bare engine, not a DI container, so no
    /// registry-backed classifier is reachable: built-in schemes only.
    link_classifier: holon_api::link_parser::LinkTargetClassifier,
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
            link_classifier: holon_api::link_parser::LinkTargetClassifier::default(),
        }
    }
}

impl BuilderServices for HeadlessBuilderServices {
    /// HeadlessBuilderServices is a test double with no backend to await: it
    /// dispatches and reports that nothing was proven, rather than
    /// inheriting a claim it cannot make.
    fn dispatch_intent_awaitable(
        &self,
        intent: holon_frontend::operations::OperationIntent,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = anyhow::Result<holon_core::Delivery>> + Send + 'static,
        >,
    > {
        self.dispatch_intent(intent);
        Box::pin(std::future::ready(Ok(holon_core::Delivery::Unproven {
            detail: "HeadlessBuilderServices: test double, no delivery to prove".to_string(),
        })))
    }

    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel {
        self.interpreter.interpret(expr, ctx, self)
    }

    /// A handle over the same engine and interpreter. `describe_ui` is the
    /// caller that needs it: reporting what the UI *should* render means
    /// materialising the lazy slots (`expand_toggle` content, tabs), which
    /// interpret through the captured services long after the build returns.
    fn clone_arc(&self) -> Arc<dyn BuilderServices> {
        Arc::new(Self {
            engine: self.engine.clone(),
            interpreter: self.interpreter.clone(),
            rt_handle: self.rt_handle.clone(),
            link_classifier: holon_api::link_parser::LinkTargetClassifier::default(),
        })
    }

    fn get_block_data(&self, _: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        (table_expr(), vec![])
    }

    fn link_classifier(&self) -> &holon_api::link_parser::LinkTargetClassifier {
        &self.link_classifier
    }

    fn resolve_profile(&self, row: &DataRow) -> Option<holon_api::RenderProfile> {
        let (profile, _computed) = self.engine.profile_resolver().resolve_with_variants(row);
        Some(profile.as_ref().clone())
    }

    fn profile_signal(&self) -> Mutable<Arc<ProfileCache>> {
        self.engine.profile_resolver().profile_signal()
    }

    /// One-shot compile + execute, delivered as a single closed batch.
    ///
    /// Headless has no CDC pump, so there is nothing to keep a subscription
    /// alive for — but the caller's Ok/Err is load-bearing: the render
    /// interpreter turns an `Err` into the block's error widget
    /// (`render_interpreter.rs`, the `live_query` arm), so a refused matview
    /// DDL or unparseable query MUST still fail here. Running the query for
    /// real is what preserves that; only the streaming half is dropped.
    ///
    /// `QueryEngine::execute_query` is the no-matview, no-CDC path, so a
    /// headless render costs one query and creates no view.
    ///
    /// `block_on` is illegal on a thread already inside a runtime and this is
    /// called from interpretation, so the wait happens on a bridge thread
    /// carrying the spawner's observability context — without it the PBT
    /// harness charges this query's SQL spans to no test scope.
    fn watch_query(
        &self,
        query: &str,
        lang: QueryLanguage,
        ctx: Option<QueryContext>,
    ) -> Result<holon_api::EnrichedChangeStream> {
        let engine = self.engine.clone();
        let rt = self.rt_handle.clone();
        let bridge = holon_frontend::bridge_thread::capture();
        let rows = std::thread::scope(|s| {
            s.spawn(|| {
                bridge.run(|| {
                    rt.block_on(holon_api::QueryEngine::execute_query(
                        engine.as_ref(),
                        query,
                        lang,
                        HashMap::new(),
                        ctx,
                    ))
                })
            })
            .join()
            .expect("headless watch_query bridge thread panicked")
        })?;

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let batch = holon_api::WithMetadata {
            inner: holon_api::Batch {
                items: rows
                    .into_iter()
                    .map(|row| holon_api::Change::Created {
                        data: holon_api::widget_spec::EnrichedRow::from_raw(row, |_| {
                            HashMap::new()
                        }),
                        origin: holon_api::ChangeOrigin::Local {
                            operation_id: None,
                            trace_id: None,
                        },
                    })
                    .collect(),
            },
            metadata: holon_api::BatchMetadata::default(),
        };
        tx.try_send(batch)
            .expect("fresh capacity-1 channel must accept its only batch");
        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
    }

    /// One-shot querying IS available headlessly even though live watching is
    /// not — `describe_ui`'s deferred-subtree expansion needs a snapshot, not a
    /// subscription.
    fn query_engine(&self) -> Option<Arc<dyn holon_api::QueryEngine>> {
        Some(self.engine.clone() as Arc<dyn holon_api::QueryEngine>)
    }

    fn widget_state(&self, _: &str) -> WidgetState {
        WidgetState::default()
    }

    fn set_widget_open(&self, _: &str, _: bool) {
        // Headless services have no UI to toggle.
    }

    fn set_widget_width(&self, _: &str, _: f32, _: bool) {
        // Headless services have no UI to resize.
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
