//! The trailing "open this nested page" chevron must survive a structural
//! rebuild (Martin dogfooding, 2026-08-08 — BugFunnel finding #5).
//!
//! A profile-driven `embedded_page` row (`hover_reveal_toggle: true`) is
//! re-synthesized on every recursive resolve, so its `expanded` `Mutable` is
//! reborn each rebuild and the ONLY thing that can carry the user's click
//! forward is the seed the builder reads:
//! `BuilderServices::block_expanded_view`. The production GPUI chevron handler
//! must therefore write THAT store — the store the widget reads — or the click
//! is discarded on the next frame and the page can never be opened.
//!
//! Both halves of the gesture are asserted, and the second is the one that was
//! red: the click flips the live VM (control), and a rebuilt VM comes back
//! OPEN (the defect).
//!
//! Run: `cargo test -p holon-gpui --test nested_page_chevron_gate`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext as FrontendRenderContext;
use holon_frontend::expand_toggle_id_for;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::StubBuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_gpui::geometry::BoundsRegistry;
use support::ReactiveFixtureView;

const GLYPH_COLLAPSED: &str = "\u{25B6}";
const GLYPH_EXPANDED: &str = "\u{25BC}";

const TARGET: &str = "nested-page";

/// The `embedded_page` render rule, verbatim in shape: a trailing
/// hover-revealed chevron over a lazily-materialised subtree.
const EMBEDDED_PAGE: &str = r#"expand_toggle(#{
    hover_reveal_toggle: true,
    header: text(col("content")),
    content: text("NESTED SUBTREE")
})"#;

/// `StubBuilderServices` plus the ONE capability under test: the view-local
/// expansion store, with `UiState`'s scheme-normalising key semantics. Shared
/// through `clone_arc` so the lazy content thunk sees the same store.
struct ExpandStoreServices {
    inner: StubBuilderServices,
    /// Own interpreter handle: `interpret` must pass THIS services impl down as
    /// `ba.services`, or the builders would read the inner stub's (empty)
    /// store.
    interpreter: Arc<holon_frontend::render_interpreter::RenderInterpreter<ReactiveViewModel>>,
    store: Arc<Mutex<HashMap<String, bool>>>,
    /// When true, `try_runtime_handle` hands out a real handle so the builder's
    /// live-follow subscription actually SPAWNS. The windowed tests leave it
    /// false (they exercise the click path and want no background writer);
    /// the live-follow tests set it, because a subscription that never runs is
    /// a line with no coverage.
    live_follow: bool,
}

impl ExpandStoreServices {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: StubBuilderServices::new(),
            interpreter: Arc::new(holon_frontend::shadow_builders::build_shadow_interpreter()),
            store: Arc::new(Mutex::new(HashMap::new())),
            live_follow: false,
        })
    }

    /// Same store, but the live-follow subscription runs.
    fn live() -> Arc<Self> {
        Arc::new(Self {
            inner: StubBuilderServices::new(),
            interpreter: Arc::new(holon_frontend::shadow_builders::build_shadow_interpreter()),
            store: Arc::new(Mutex::new(HashMap::new())),
            live_follow: true,
        })
    }

    fn stored(&self) -> HashMap<String, bool> {
        self.store.lock().unwrap().clone()
    }
}

fn bare(target_id: &str) -> &str {
    target_id.strip_prefix("block:").unwrap_or(target_id)
}

impl BuilderServices for ExpandStoreServices {
    fn interpret(&self, expr: &RenderExpr, ctx: &FrontendRenderContext) -> ReactiveViewModel {
        self.interpreter.interpret(expr, ctx, self)
    }
    fn clone_arc(&self) -> Arc<dyn BuilderServices> {
        Arc::new(Self {
            inner: StubBuilderServices::new(),
            interpreter: self.interpreter.clone(),
            store: self.store.clone(),
            live_follow: self.live_follow,
        })
    }
    fn get_block_data(
        &self,
        id: &holon_api::EntityUri,
    ) -> (RenderExpr, Vec<Arc<holon_api::widget_spec::DataRow>>) {
        self.inner.get_block_data(id)
    }
    fn link_classifier(&self) -> &holon_api::link_parser::LinkTargetClassifier {
        self.inner.link_classifier()
    }
    fn resolve_profile(
        &self,
        row: &holon_api::widget_spec::DataRow,
    ) -> Option<holon_api::RowProfile> {
        self.inner.resolve_profile(row)
    }
    fn watch_query(
        &self,
        query: &str,
        lang: holon_api::QueryLanguage,
        ctx: Option<holon_frontend::QueryContext>,
    ) -> anyhow::Result<holon_api::EnrichedChangeStream> {
        self.inner.watch_query(query, lang, ctx)
    }
    fn widget_state(&self, id: &str) -> holon_frontend::WidgetState {
        self.inner.widget_state(id)
    }
    fn set_widget_open(&self, id: &str, open: bool) {
        self.inner.set_widget_open(id, open)
    }
    fn dispatch_intent(&self, intent: holon_frontend::operations::OperationIntent) {
        self.inner.dispatch_intent(intent)
    }
    fn present_op(
        &self,
        op: holon_api::render_types::OperationDescriptor,
        ctx_params: HashMap<String, Value>,
    ) {
        self.inner.present_op(op, ctx_params)
    }
    fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.inner.runtime_handle()
    }
    fn try_runtime_handle(&self) -> Option<tokio::runtime::Handle> {
        self.live_follow.then(|| self.inner.runtime_handle())
    }
    fn search_link_candidates(
        &self,
        q: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = anyhow::Result<Vec<holon_api::LinkCandidate>>>
                + Send
                + 'static,
        >,
    > {
        self.inner.search_link_candidates(q)
    }
    fn block_expanded_view(&self, target_id: &str) -> Option<bool> {
        self.store.lock().unwrap().get(bare(target_id)).copied()
    }
    fn set_block_expanded_view(&self, target_id: &str, expanded: bool) {
        self.store
            .lock()
            .unwrap()
            .insert(bare(target_id).to_string(), expanded);
    }
}

/// A fresh structural build of the embedded-page row — exactly what a
/// recursive resolve produces on every snapshot.
fn build(services: &Arc<ExpandStoreServices>) -> Arc<ReactiveViewModel> {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr =
        holon_api::render_dsl::parse_render_dsl(EMBEDDED_PAGE).expect("embedded_page DSL parses");
    let mut row = DataRow::new();
    row.insert("id".into(), Value::String(format!("block:{TARGET}")));
    row.insert("content".into(), Value::String("A Nested Page".into()));
    let ctx = FrontendRenderContext::default().with_row(Arc::new(row));
    Arc::new(services.interpret(&expr, &ctx))
}

struct Painted {
    glyph: String,
    center: gpui::Point<gpui::Pixels>,
}

/// Render `vm` in a real window, click the trailing chevron, and report what
/// the chevron painted BEFORE the click.
fn click_chevron(
    cx: &mut TestAppContext,
    vm: Arc<ReactiveViewModel>,
    services: Arc<dyn BuilderServices>,
) -> Painted {
    let bounds = BoundsRegistry::new();
    let (_view, mut vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| {
            ReactiveFixtureView::with_services_and_bounds(
                vm,
                services,
                size(px(800.0), px(600.0)),
                bounds,
            )
        }
    });
    vcx.run_until_parked();
    bounds.flush();

    let el_id = expand_toggle_id_for(TARGET);
    let info = bounds
        .all_elements()
        .into_iter()
        .find(|(id, _)| *id == el_id)
        .map(|(_, i)| i)
        .unwrap_or_else(|| panic!("no chevron registered under {el_id}"));
    let glyph = info
        .displayed_text
        .as_deref()
        .unwrap_or_else(|| {
            panic!("the chevron must record the glyph it paints; open-vs-closed is unreadable")
        })
        .to_string();
    let center = gpui::point(
        px(info.x + info.width / 2.0),
        px(info.y + info.height / 2.0),
    );

    vcx.simulate_mouse_move(center, None, Default::default());
    vcx.simulate_click(center, Default::default());
    vcx.run_until_parked();

    Painted { glyph, center }
}

/// The painted glyph of a freshly-rendered VM.
fn painted_glyph(
    cx: &mut TestAppContext,
    vm: Arc<ReactiveViewModel>,
    services: Arc<dyn BuilderServices>,
) -> String {
    let bounds = BoundsRegistry::new();
    let (_view, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| {
            ReactiveFixtureView::with_services_and_bounds(
                vm,
                services,
                size(px(800.0), px(600.0)),
                bounds,
            )
        }
    });
    vcx.run_until_parked();
    bounds.flush();
    let el_id = expand_toggle_id_for(TARGET);
    bounds
        .all_elements()
        .into_iter()
        .find(|(id, _)| *id == el_id)
        .and_then(|(_, i)| i.displayed_text.as_deref().map(str::to_string))
        .unwrap_or_else(|| panic!("no chevron glyph registered under {el_id}"))
}

/// CONTROL: the click reaches the production handler and flips the live row.
/// Green before and after the fix — it is what proves the red below is about
/// the store, not about a missed hit-test.
#[gpui::test]
fn chevron_click_opens_the_live_row(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let services = ExpandStoreServices::new();
    let vm = build(&services);
    let before = click_chevron(cx, vm.clone(), services.clone());
    assert_eq!(
        before.glyph, GLYPH_COLLAPSED,
        "a nested page starts collapsed; clicked at {:?}",
        before.center
    );
    assert!(
        vm.expanded.as_ref().expect("expand_toggle gate").get(),
        "the chevron click must open the row it was clicked on"
    );
}

/// THE DEFECT: the page must still be open after the structural rebuild that
/// a recursive resolve performs on the very next snapshot. Red while the
/// handler writes only `set_field(collapsed)` and the gate seeds from
/// `block_expanded_view`.
#[gpui::test]
fn chevron_click_survives_the_structural_rebuild(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let services = ExpandStoreServices::new();
    click_chevron(cx, build(&services), services.clone());

    let rebuilt = build(&services);
    assert!(
        rebuilt.expanded.as_ref().expect("expand_toggle gate").get(),
        "the rebuilt row re-seeded CLOSED: the gate reads a store the production chevron handler \
         never writes, so every resolve discards the click"
    );
    assert!(
        rebuilt
            .lazy_slot
            .as_ref()
            .expect("expand_toggle carries a lazy content slot")
            .materialize_if_gated()
            .is_some(),
        "an open nested page must materialise its subtree"
    );
    assert_eq!(
        painted_glyph(cx, rebuilt, services),
        GLYPH_EXPANDED,
        "the rebuilt row must paint the open chevron"
    );
}

// ── The live-follow subscription: which `collapsed` edges are DURABLE ────
//
// The nested-page gate and the outline fold are two different affordances over
// one row. `collapsed` belongs to the LEADING `tree_item` chevron; the view
// store belongs to the TRAILING nested-page chevron. The subscription that
// follows external `collapsed` changes may therefore only propagate the FOLD
// direction: a remote fold closes what it hides, but a remote UNFOLD of the
// outline row must not open — and eagerly materialise — a nested page nobody
// clicked.

/// Build against a caller-owned row cell, so a test can drive an external
/// `collapsed` edge the way CDC does.
fn build_over(
    services: &Arc<ExpandStoreServices>,
    row: &futures_signals::signal::Mutable<Arc<DataRow>>,
) -> Arc<ReactiveViewModel> {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr =
        holon_api::render_dsl::parse_render_dsl(EMBEDDED_PAGE).expect("embedded_page DSL parses");
    let ctx = FrontendRenderContext::default().with_row_mutable(row.read_only());
    Arc::new(services.interpret(&expr, &ctx))
}

fn row_with(collapsed: bool) -> Arc<DataRow> {
    let mut row = DataRow::new();
    row.insert("id".into(), Value::String(format!("block:{TARGET}")));
    row.insert("content".into(), Value::String("A Nested Page".into()));
    row.insert("collapsed".into(), Value::Integer(i64::from(collapsed)));
    Arc::new(row)
}

/// Let the spawned subscription observe the edge.
async fn settle() {
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}

/// THE REGRESSION the round-1 fix introduced: unfolding the OUTLINE row (the
/// leading `tree_item` chevron, which owns `collapsed`) must not open the
/// TRAILING nested-page gate. A durable unfold write turns every outline
/// unfold into a persistent eager subtree load — the exact "auto-expand every
/// nested page" class that ruled out seeding the gate from `collapsed`.
#[tokio::test]
async fn an_external_unfold_leaves_the_nested_page_closed() {
    let services = ExpandStoreServices::live();
    let row = futures_signals::signal::Mutable::new(row_with(true));
    let vm = build_over(&services, &row);
    assert!(
        !vm.expanded.as_ref().expect("gate").get(),
        "a folded row starts with its nested page closed"
    );

    // External unfold of the OUTLINE row: collapsed 1 -> 0.
    row.set(row_with(false));
    settle().await;

    assert_eq!(
        services.stored(),
        HashMap::new(),
        "unfolding the outline row must write NOTHING to the nested-page view \
         store — that store is the trailing chevron's, and a durable entry here \
         opens a page the user never clicked"
    );

    let rebuilt = build_over(&services, &row);
    assert!(
        !rebuilt.expanded.as_ref().expect("gate").get(),
        "the rebuilt row must still be closed after an external unfold"
    );
    assert!(
        rebuilt
            .lazy_slot
            .as_ref()
            .expect("lazy slot")
            .materialize_if_gated()
            .is_none(),
        "an unclicked nested page must not materialise its subtree"
    );
}

/// The direction that IS durable: a remote fold closes the nested page, and the
/// close survives the rebuild. Without this the fold half would be unasserted
/// and a directional guard could silently drop it.
#[tokio::test]
async fn an_external_fold_closes_the_nested_page_across_a_rebuild() {
    let services = ExpandStoreServices::live();
    let row = futures_signals::signal::Mutable::new(row_with(false));
    // The user opened it — exactly what the production chevron handler records.
    services.set_block_expanded_view(TARGET, true);
    let vm = build_over(&services, &row);
    assert!(
        vm.expanded.as_ref().expect("gate").get(),
        "the clicked page starts open"
    );

    // External fold: collapsed 0 -> 1.
    row.set(row_with(true));
    settle().await;

    assert_eq!(
        services.stored().get(TARGET),
        Some(&false),
        "a remote fold must close the nested page durably, or the rebuild would \
         re-open it from the stale click"
    );
    let rebuilt = build_over(&services, &row);
    assert!(
        !rebuilt.expanded.as_ref().expect("gate").get(),
        "the rebuilt row must be closed after an external fold"
    );
}
