//! Windowed rung for the `input_box` compose widget.
//!
//! `input_box` is the chat compose box: an ephemeral draft buffer that fires a
//! declaratively wired PN operation on submit, passing the typed text as the
//! operation's `modified_param`. Every other actionable widget dispatches an
//! `OperationDescriptor` (ADR 0024), and so must this one — there is no side
//! channel for "send a message".
//!
//! What the rung drives, through a real GPUI window and the production
//! `builders::render` pipeline: click the box to focus it, type, press Enter,
//! and observe what reached `BuilderServices::dispatch_intent_awaitable`.
//!
//! The three properties under test:
//!   1. submit fires the wired op with the draft text bound to `modified_param`
//!   2. a successful dispatch CLEARS the draft
//!   3. a failed dispatch KEEPS it — the user never loses typed text
//!
//! Run: cargo test -p holon-gpui --features pbt --test
//! input_box_windowed -- --test-threads=1

mod support;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use gpui::TestAppContext;
use gpui::VisualTestContext;
use gpui::prelude::*;
use gpui::px;
use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::Value;
use holon_api::render_dsl::parse_render_dsl;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::QueryContext;
use holon_frontend::RenderContext;
use holon_frontend::WidgetState;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_gpui::entity_view_registry::LocalEntityScope;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::render::builders::GpuiRenderContext;
use holon_gpui::render::builders::{self};

/// A compose box wired to a harmless stub operation. `navigation.focus` stands
/// in for the real send op (later increment); what matters is that a declared
/// `action:` template becomes an `OperationWiring` and that `text_param` names
/// the parameter the typed text rides in.
const SRC: &str = "input_box(#{placeholder: \"Message\", submit_label: \"Send\", \
                   text_param: \"content\", action: navigation_focus(#{region: \"main\"})})";

const TYPED: &str = "hello";

// ── Recording services ─────────────────────────────────────────────────

/// `BuilderServices` that records every awaited dispatch and can be armed to
/// fail the next one. The eight required trait methods delegate to the shared
/// `StubBuilderServices`; only the dispatch seam is ours.
struct RecordingServices {
    inner: holon_frontend::reactive::StubBuilderServices,
    /// The widget hands its dispatch to `runtime_handle()`; the stub's shared
    /// runtime is never driven inside a gpui test, so the seam owns a live
    /// multi-thread runtime that actually runs the task.
    rt: tokio::runtime::Runtime,
    dispatched: Mutex<Vec<OperationIntent>>,
    fail: AtomicBool,
}

impl RecordingServices {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: holon_frontend::reactive::StubBuilderServices::new(),
            rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("recording services runtime"),
            dispatched: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
        })
    }

    /// Run the queued dispatch task ON THE TEST THREAD. gpui's `TestScheduler`
    /// panics the moment its executor is woken from another thread, so the
    /// widget's `runtime_handle().spawn(...)` must be driven here rather than
    /// by a worker.
    fn pump(&self) {
        self.rt.block_on(async {
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
        });
    }

    fn arm_failure(&self) {
        self.fail.store(true, Ordering::SeqCst);
    }

    fn dispatched(&self) -> Vec<OperationIntent> {
        self.dispatched.lock().unwrap().clone()
    }

    fn record(&self, intent: OperationIntent) -> anyhow::Result<()> {
        self.dispatched.lock().unwrap().push(intent);
        if self.fail.swap(false, Ordering::SeqCst) {
            anyhow::bail!("recording services: dispatch armed to fail");
        }
        Ok(())
    }
}

impl BuilderServices for RecordingServices {
    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel {
        self.inner.interpret(expr, ctx)
    }
    /// Loud rather than delegating: a handle to the inner stub would drop this
    /// fixture's dispatch record, so the assertions would read an empty log.
    fn clone_arc(&self) -> Arc<dyn BuilderServices> {
        unimplemented!("RecordingServices::clone_arc — a handle would drop the dispatch record")
    }
    fn get_block_data(&self, id: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        self.inner.get_block_data(id)
    }
    fn link_classifier(&self) -> &holon_api::link_parser::LinkTargetClassifier {
        self.inner.link_classifier()
    }
    fn resolve_profile(&self, row: &DataRow) -> Option<holon_api::RenderProfile> {
        self.inner.resolve_profile(row)
    }
    fn watch_query(
        &self,
        query: &str,
        lang: QueryLanguage,
        ctx: Option<QueryContext>,
    ) -> anyhow::Result<holon_api::EnrichedChangeStream> {
        self.inner.watch_query(query, lang, ctx)
    }
    fn widget_state(&self, id: &str) -> WidgetState {
        self.inner.widget_state(id)
    }
    fn search_link_candidates(
        &self,
        filter: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = anyhow::Result<Vec<holon_api::LinkCandidate>>>
                + Send
                + 'static,
        >,
    > {
        self.inner.search_link_candidates(filter)
    }
    fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.rt.handle().clone()
    }
    fn dispatch_intent(&self, intent: OperationIntent) {
        let _ = self.record(intent);
    }
    fn dispatch_intent_awaitable(
        &self,
        intent: OperationIntent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'static>>
    {
        Box::pin(std::future::ready(self.record(intent)))
    }
    fn present_op(
        &self,
        op: holon_api::render_types::OperationDescriptor,
        ctx_params: HashMap<String, Value>,
    ) {
        self.inner.present_op(op, ctx_params)
    }
}

// ── Fixture ────────────────────────────────────────────────────────────

/// Renders one interpreted ViewModel through the production
/// `builders::render`. Separate from `support::ReactiveFixtureView` only
/// because a text input needs the window's first layer to be a
/// `gpui_component::Root`.
struct ComposeFixture {
    vm: Arc<ReactiveViewModel>,
    services: Arc<dyn BuilderServices>,
    bounds: BoundsRegistry,
    cache: holon_gpui::entity_view_registry::EntityCache,
}

impl gpui::Render for ComposeFixture {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        self.bounds.begin_pass();
        let gctx = GpuiRenderContext::new(
            RenderContext::default(),
            self.services.clone(),
            self.bounds.clone(),
            LocalEntityScope::new().with_cache(self.cache.clone()),
            NavigationState::new(),
            window,
            cx,
        );
        gpui::div()
            .size_full()
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(builders::render(&self.vm, &gctx))
    }
}

struct Rig {
    vm: Arc<ReactiveViewModel>,
    services: Arc<RecordingServices>,
    bounds: BoundsRegistry,
}

fn mount(cx: &mut TestAppContext) -> (Rig, &mut VisualTestContext) {
    cx.update(|cx| gpui_component::init(cx));
    let services = RecordingServices::new();
    let dyn_services: Arc<dyn BuilderServices> = services.clone();

    let expr = parse_render_dsl(SRC).expect("input_box src parses");
    let interp = holon_frontend::shadow_builders::build_shadow_interpreter();
    let vm = Arc::new(interp.interpret(&expr, &RenderContext::default(), &*dyn_services));

    let bounds = BoundsRegistry::new();
    let (_root, vcx) = cx.add_window_view({
        let vm = vm.clone();
        let services = dyn_services.clone();
        let bounds = bounds.clone();
        move |window, cx| {
            let fixture = cx.new(|_| ComposeFixture {
                vm,
                services,
                bounds,
                cache: Default::default(),
            });
            gpui_component::Root::new(fixture, window, cx)
        }
    });
    vcx.run_until_parked();
    bounds.flush();
    (
        Rig {
            vm,
            services,
            bounds,
        },
        vcx,
    )
}

/// The live draft the widget owns. Absent until `input_box` exists.
fn draft(rig: &Rig) -> Option<String> {
    rig.vm.draft.as_ref().map(|d| d.get_cloned())
}

/// Click the centre of the rendered `input_box` so it takes window focus,
/// then type `text` and press Enter — the real user gesture.
fn compose_and_submit(vcx: &mut VisualTestContext, rig: &Rig, text: &str) {
    let target = rig
        .bounds
        .all_elements()
        .into_iter()
        .find(|(_, info)| &*info.widget_type == "input_box")
        .map(|(_, info)| {
            gpui::point(
                px(info.x + info.width / 2.0),
                px(info.y + info.height / 2.0),
            )
        });
    // Deliberately tolerant: with the widget missing there is nothing to click,
    // and the gesture must still run so the ASSERTIONS below are what fails —
    // an `expect` here would report scaffolding, not the missing behavior.
    if let Some(position) = target {
        vcx.simulate_mouse_move(position, None, Default::default());
        vcx.simulate_click(position, Default::default());
        vcx.run_until_parked();
    }
    if !text.is_empty() {
        let per_char: String = text
            .chars()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        vcx.simulate_keystrokes(&per_char);
        vcx.run_until_parked();
    }
    vcx.simulate_keystrokes("enter");
    // Dispatch hops to the op runtime and its result hops back onto gpui's
    // executor. Alternate the two, on this one thread, until both are idle.
    for _ in 0..20 {
        rig.services.pump();
        vcx.run_until_parked();
    }
}

// ── Properties ─────────────────────────────────────────────────────────

/// PRIMARY. Typing into the compose box and pressing Enter fires the wired
/// operation ONCE, carrying the declared `bound_params` plus the typed text
/// under the wiring's `modified_param`.
#[gpui::test]
fn submit_dispatches_wired_op_with_typed_text_as_modified_param(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    compose_and_submit(vcx, &rig, TYPED);

    let dispatched = rig.services.dispatched();
    assert_eq!(
        dispatched.len(),
        1,
        "typing + Enter must fire the wired op exactly once; dispatched: {dispatched:?}"
    );
    let intent = &dispatched[0];
    assert_eq!(intent.entity_name.as_str(), "navigation");
    assert_eq!(intent.op_name, "focus");
    assert_eq!(
        intent.params.get("content"),
        Some(&Value::String(TYPED.to_string())),
        "the typed text must ride in the wiring's modified_param; params: {:?}",
        intent.params
    );
    assert_eq!(
        intent.params.get("region"),
        Some(&Value::String("main".to_string())),
        "declared bound_params must survive alongside the draft text"
    );
}

/// A successful dispatch clears the compose buffer — the box is ready for the
/// next message.
#[gpui::test]
fn successful_submit_clears_the_draft(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    compose_and_submit(vcx, &rig, TYPED);

    assert_eq!(
        rig.services.dispatched().len(),
        1,
        "precondition: the submit must have dispatched"
    );
    assert_eq!(
        draft(&rig),
        Some(String::new()),
        "a successful dispatch must clear the draft"
    );
}

/// A FAILED dispatch keeps the buffer. Losing what the user typed because the
/// backend rejected the op is the one outcome a compose box must never have.
#[gpui::test]
fn failed_submit_keeps_the_draft(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    rig.services.arm_failure();
    compose_and_submit(vcx, &rig, TYPED);

    assert_eq!(
        rig.services.dispatched().len(),
        1,
        "precondition: the submit must have reached dispatch"
    );
    assert_eq!(
        draft(&rig),
        Some(TYPED.to_string()),
        "a failed dispatch must leave the typed text in the box"
    );
}

/// Enter on an empty box sends nothing. A regression guard rather than a
/// red-first property — with the widget absent it holds vacuously.
#[gpui::test]
fn enter_on_an_empty_box_dispatches_nothing(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    compose_and_submit(vcx, &rig, "");

    assert!(
        rig.services.dispatched().is_empty(),
        "an empty compose box must not send: {:?}",
        rig.services.dispatched()
    );
}
