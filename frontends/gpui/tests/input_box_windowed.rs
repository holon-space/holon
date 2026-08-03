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

/// Two compose boxes side by side, wired to the SAME operation and carrying no
/// row id — the shape under which any identity derived from (row, op name)
/// degenerates to one key for both.
const SRC_TWO: &str = "row(input_box(#{placeholder: \"A\", text_param: \"content\", \
                       action: navigation_focus(#{region: \"main\"})}), \
                       input_box(#{placeholder: \"B\", text_param: \"content\", \
                       action: navigation_focus(#{region: \"main\"})}))";

const TYPED: &str = "hello";

/// The verbatim text `RecordingServices` fails with. The compose box must
/// surface exactly this, not a paraphrase of it.
const FAILURE_TEXT: &str = "recording services: dispatch armed to fail";

/// The verbatim wording an UNPROVEN ack carries. The strip must show exactly
/// this, and must never rewrite it into "failed".
const UNPROVEN_TEXT: &str =
    "'send_message' acked `outcome: unconfirmed` - dispatched, but delivery is NOT proven";

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
    unproven: AtomicBool,
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
            unproven: AtomicBool::new(false),
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

    /// Arm the next dispatch to succeed with delivery NOT proven — the
    /// provider's `{"outcome":"unconfirmed"}` as it reaches the widget.
    fn arm_unproven(&self) {
        self.unproven.store(true, Ordering::SeqCst);
    }

    fn dispatched(&self) -> Vec<OperationIntent> {
        self.dispatched.lock().unwrap().clone()
    }

    fn record(&self, intent: OperationIntent) -> anyhow::Result<holon_core::Delivery> {
        self.dispatched.lock().unwrap().push(intent);
        if self.fail.swap(false, Ordering::SeqCst) {
            anyhow::bail!(FAILURE_TEXT);
        }
        if self.unproven.swap(false, Ordering::SeqCst) {
            return Ok(holon_core::Delivery::Unproven {
                detail: UNPROVEN_TEXT.to_string(),
            });
        }
        Ok(holon_core::Delivery::Proven)
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
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = anyhow::Result<holon_core::Delivery>> + Send + 'static,
        >,
    > {
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
    src: &'static str,
}

impl ComposeFixture {
    /// Re-run interpretation and push the fresh tree down through
    /// `with_update` — the production structural-rebuild path (`ReactiveShell`
    /// does the same on every re-interpretation).
    fn reinterpret(&mut self) {
        let expr = parse_render_dsl(self.src).expect("input_box src parses");
        let interp = holon_frontend::shadow_builders::build_shadow_interpreter();
        let fresh = interp.interpret(&expr, &RenderContext::default(), &*self.services);
        self.vm = Arc::new(self.vm.with_update(&fresh));
    }
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
    fixture: gpui::Entity<ComposeFixture>,
    cache: holon_gpui::entity_view_registry::EntityCache,
}

fn mount(cx: &mut TestAppContext) -> (Rig, &mut VisualTestContext) {
    mount_src(cx, SRC)
}

fn mount_src<'a>(
    cx: &'a mut TestAppContext,
    src: &'static str,
) -> (Rig, &'a mut VisualTestContext) {
    cx.update(|cx| gpui_component::init(cx));
    let services = RecordingServices::new();
    let dyn_services: Arc<dyn BuilderServices> = services.clone();

    let expr = parse_render_dsl(src).expect("input_box src parses");
    let interp = holon_frontend::shadow_builders::build_shadow_interpreter();
    let vm = Arc::new(interp.interpret(&expr, &RenderContext::default(), &*dyn_services));

    let bounds = BoundsRegistry::new();
    let cache: holon_gpui::entity_view_registry::EntityCache = Default::default();
    let slot: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<ComposeFixture>>>> =
        Default::default();
    let (_root, vcx) = cx.add_window_view({
        let vm = vm.clone();
        let services = dyn_services.clone();
        let bounds = bounds.clone();
        let cache = cache.clone();
        let slot = slot.clone();
        move |window, cx| {
            let fixture = cx.new(|_| ComposeFixture {
                vm,
                services,
                bounds,
                cache,
                src,
            });
            *slot.borrow_mut() = Some(fixture.clone());
            gpui_component::Root::new(fixture, window, cx)
        }
    });
    vcx.run_until_parked();
    bounds.flush();
    let fixture = slot.borrow_mut().take().expect("fixture entity captured");
    (
        Rig {
            vm,
            services,
            bounds,
            fixture,
            cache,
        },
        vcx,
    )
}

/// The live draft the widget owns. Absent until `input_box` exists.
fn draft(rig: &Rig) -> Option<String> {
    rig.vm.draft.as_ref().map(|d| d.get_cloned())
}

/// The GPUI entity ids of every cached compose view, sorted. Distinguishes
/// "one view reused" from "one view SHARED by two nodes".
fn view_ids(rig: &Rig) -> Vec<gpui::EntityId> {
    let mut ids: Vec<_> = rig
        .cache
        .read()
        .unwrap()
        .values()
        .map(|e| e.entity_id())
        .collect();
    ids.sort();
    ids
}

/// The rendered compose boxes in visual order (top-to-bottom, then
/// left-to-right).
fn box_centers(rig: &Rig) -> Vec<gpui::Point<gpui::Pixels>> {
    let mut boxes: Vec<_> = rig
        .bounds
        .all_elements()
        .into_iter()
        .map(|(_, info)| info)
        .filter(|info| &*info.widget_type == "input_box")
        .collect();
    boxes.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).expect("finite bounds"));
    boxes
        .into_iter()
        .map(|info| {
            gpui::point(
                px(info.x + info.width / 2.0),
                px(info.y + info.height / 2.0),
            )
        })
        .collect()
}

/// Click the centre of compose box `index` so it takes window focus.
///
/// Deliberately tolerant: with the widget missing there is nothing to click,
/// and the gesture must still run so the ASSERTIONS are what fail — an
/// `expect` here would report scaffolding, not the missing behavior.
fn focus_box(vcx: &mut VisualTestContext, rig: &Rig, index: usize) {
    if let Some(position) = box_centers(rig).get(index).copied() {
        vcx.simulate_mouse_move(position, None, Default::default());
        vcx.simulate_click(position, Default::default());
        vcx.run_until_parked();
    }
}

fn type_text(vcx: &mut VisualTestContext, text: &str) {
    if text.is_empty() {
        return;
    }
    let per_char: String = text
        .chars()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    vcx.simulate_keystrokes(&per_char);
    vcx.run_until_parked();
}

/// Press Enter and let GPUI's executor drain, but do NOT drive the op runtime
/// — the submit reaches its dispatch hop and parks there. This is what a
/// second Enter arriving "before the first dispatch resolves" means.
fn press_enter(vcx: &mut VisualTestContext) {
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
}

/// Dispatch hops to the op runtime and its result hops back onto gpui's
/// executor. Alternate the two, on this one thread, until both are idle.
fn settle(vcx: &mut VisualTestContext, rig: &Rig) {
    for _ in 0..20 {
        rig.services.pump();
        vcx.run_until_parked();
    }
    // Commit the last render pass so what the box painted is readable.
    rig.bounds.flush();
}

/// Click the compose box so it takes window focus, then type `text` and press
/// Enter — the real user gesture.
fn compose_and_submit(vcx: &mut VisualTestContext, rig: &Rig, text: &str) {
    focus_box(vcx, rig, 0);
    type_text(vcx, text);
    press_enter(vcx);
    settle(vcx, rig);
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

/// PRIMARY (in-flight guard). Two Enters pressed before the first dispatch
/// resolves must produce ONE dispatch, not two. The wired op is `once_only`:
/// a second dispatch is a duplicated irreversible external effect, and no
/// later increment can take it back.
#[gpui::test]
fn a_second_enter_while_in_flight_dispatches_nothing(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    focus_box(vcx, &rig, 0);
    type_text(vcx, TYPED);

    // The op runtime is not driven between these two — the first submit is
    // still parked on its dispatch hop when the second Enter arrives.
    press_enter(vcx);
    press_enter(vcx);
    settle(vcx, &rig);

    let dispatched = rig.services.dispatched();
    assert_eq!(
        dispatched.len(),
        1,
        "a submit while one is in flight must be inert; dispatched: {dispatched:?}"
    );
}

/// The guard belongs to the NODE, not to the view. Drop the cached compose
/// view while a send is in flight and the recreated one must still refuse to
/// send: a guard that comes back `Idle` with a fresh view is not a guard.
#[gpui::test]
fn the_in_flight_guard_outlives_its_view(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    focus_box(vcx, &rig, 0);
    type_text(vcx, TYPED);
    press_enter(vcx);

    rig.cache.write().unwrap().clear();
    vcx.update(|_, cx| {
        rig.fixture.update(cx, |f, cx| {
            f.reinterpret();
            cx.notify();
        })
    });
    vcx.run_until_parked();
    rig.bounds.flush();

    focus_box(vcx, &rig, 0);
    press_enter(vcx);
    settle(vcx, &rig);

    let dispatched = rig.services.dispatched();
    assert_eq!(
        dispatched.len(),
        1,
        "a compose view rebuilt mid-flight must not re-send; dispatched: {dispatched:?}"
    );
}

/// A failed dispatch surfaces the provider's error text VERBATIM on a
/// `CommandFailed` toast. A compose box that swallows the reason, or
/// paraphrases it into "send failed", leaves the user with no way to tell a
/// typo from an outage.
#[gpui::test]
fn failed_submit_toasts_the_provider_error_verbatim(cx: &mut TestAppContext) {
    let toasts: Arc<Mutex<Vec<holon_gpui::share_ui::DegradedToast>>> = Default::default();
    cx.update(|cx| {
        let sink = toasts.clone();
        cx.set_global(holon_gpui::share_ui::DegradedToastSink::new(move |t, _| {
            sink.lock().unwrap().push(t)
        }));
    });

    let (rig, vcx) = mount(cx);
    rig.services.arm_failure();
    compose_and_submit(vcx, &rig, TYPED);

    let got = toasts.lock().unwrap().clone();
    assert_eq!(
        got.len(),
        1,
        "a failed dispatch must raise exactly one degraded toast; got: {got:?}"
    );
    assert_eq!(
        got[0].kind,
        holon_gpui::share_ui::DegradedKind::CommandFailed,
        "a rejected compose submit is a failed command"
    );
    assert_eq!(
        got[0].detail, FAILURE_TEXT,
        "the provider's error text must reach the toast verbatim"
    );
}

/// A structural rebuild must not lose what the user has typed, and must not
/// throw away the live text surface underneath it. This is the property that
/// justified NOT reusing `editable_text`, whose content is re-derived from a
/// row column on every CDC write.
#[gpui::test]
fn draft_and_text_surface_survive_a_structural_rebuild(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    focus_box(vcx, &rig, 0);
    type_text(vcx, TYPED);
    assert_eq!(
        draft(&rig),
        Some(TYPED.to_string()),
        "precondition: typing must reach the draft"
    );
    let before = view_ids(&rig);

    let rebuilt = vcx.update(|_, cx| {
        rig.fixture.update(cx, |f, cx| {
            f.reinterpret();
            cx.notify();
            f.vm.clone()
        })
    });
    vcx.run_until_parked();
    rig.bounds.flush();

    assert_eq!(
        rebuilt.draft.as_ref().map(|d| d.get_cloned()),
        Some(TYPED.to_string()),
        "a structural rebuild must carry the compose buffer onto the fresh node"
    );
    assert_eq!(
        view_ids(&rig),
        before,
        "the rebuild must reuse the SAME compose view — a fresh one drops the \
         caret, the selection, and any in-progress IME composition"
    );
}

/// Two compose boxes on screen at once are two independent buffers. They carry
/// no row id and share an operation name, so anything that keys their view on
/// (row, op) hands both of them one box.
#[gpui::test]
fn sibling_compose_boxes_do_not_share_a_buffer(cx: &mut TestAppContext) {
    let (rig, vcx) = mount_src(cx, SRC_TWO);
    let drafts: Vec<_> = rig
        .vm
        .children
        .iter()
        .map(|c| c.draft.clone().expect("each input_box carries a draft"))
        .collect();
    assert_eq!(drafts.len(), 2, "fixture must render two sibling boxes");
    assert_eq!(
        box_centers(&rig).len(),
        2,
        "both sibling boxes must reach the screen"
    );
    assert_eq!(
        view_ids(&rig).len(),
        2,
        "two boxes must own two views; one cached view means one shared buffer"
    );

    focus_box(vcx, &rig, 1);
    type_text(vcx, "world");

    assert_eq!(
        drafts[1].get_cloned(),
        "world",
        "typing into the second box must land in ITS draft"
    );
    assert_eq!(
        drafts[0].get_cloned(),
        "",
        "the first box must not receive the second box's text"
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

// ── Three-state send feedback ──────────────────────────────────────────

/// Every send-status strip on screen, as (widget tag, painted text).
fn strips(rig: &Rig) -> Vec<(String, String)> {
    let mut found: Vec<_> = rig
        .bounds
        .all_elements()
        .into_iter()
        .map(|(_, info)| info)
        .filter(|info| info.widget_type.starts_with("send_status"))
        .map(|info| {
            (
                info.widget_type.to_string(),
                info.displayed_text
                    .as_deref()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    found.sort();
    found
}

/// PRIMARY (rendering). An UNPROVEN ack must reach the screen as its OWN
/// state: not a chat bubble (which would fabricate a record of a message that
/// may never have arrived), not the word "failed" (which invites the resend
/// this transport cannot make safe), and with no retry affordance at all.
#[gpui::test]
fn an_unproven_ack_renders_as_a_pending_send_strip(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    rig.services.arm_unproven();
    compose_and_submit(vcx, &rig, TYPED);

    let on_screen = strips(&rig);
    assert_eq!(
        on_screen.len(),
        1,
        "an unproven ack must show exactly one send-status strip; got {on_screen:?}"
    );
    let (tag, text) = &on_screen[0];
    assert_eq!(
        tag, "send_status_unconfirmed",
        "the strip must declare WHICH state it reports"
    );
    assert!(
        text.contains("sent, not acknowledged"),
        "the strip must name the state in those words; got: {text}"
    );
    assert!(
        !text.to_lowercase().contains("fail"),
        "an unproven send is not a failure and must not be worded as one; got: {text}"
    );
    assert!(
        text.contains(UNPROVEN_TEXT),
        "the provider's own wording must appear verbatim; got: {text}"
    );
    assert!(
        text.contains(TYPED),
        "the strip must record WHAT was sent unproven; got: {text}"
    );

    let bubbles = rig
        .bounds
        .all_elements()
        .into_iter()
        .filter(|(_, i)| i.widget_type.as_ref() == "chat_bubble")
        .count();
    assert_eq!(
        bubbles, 0,
        "an unproven message must never be rendered as a plain chat bubble"
    );

    assert_eq!(
        draft(&rig),
        Some(TYPED.to_string()),
        "an unproven send must keep the typed text — the user has no other copy"
    );
}

/// The three outcomes are told apart on screen. Delivered shows no strip at
/// all (the box is clear and ready); unproven and refused show DIFFERENT
/// strips with different wording.
#[gpui::test]
fn the_three_send_outcomes_render_distinctly(cx: &mut TestAppContext) {
    let (delivered, vcx) = mount(cx);
    compose_and_submit(vcx, &delivered, TYPED);
    let delivered_strips = strips(&delivered);
    assert!(
        delivered_strips.is_empty(),
        "a proven delivery leaves no pending-send strip; got {delivered_strips:?}"
    );
    assert_eq!(
        draft(&delivered),
        Some(String::new()),
        "only a proven delivery clears the draft"
    );

    let (unproven, vcx) = mount(cx);
    unproven.services.arm_unproven();
    compose_and_submit(vcx, &unproven, TYPED);
    let unproven_strips = strips(&unproven);

    let (refused, vcx) = mount(cx);
    refused.services.arm_failure();
    compose_and_submit(vcx, &refused, TYPED);
    let refused_strips = strips(&refused);

    assert_eq!(unproven_strips.len(), 1, "got {unproven_strips:?}");
    assert_eq!(refused_strips.len(), 1, "got {refused_strips:?}");
    assert_ne!(
        unproven_strips[0].0, refused_strips[0].0,
        "sent-not-acknowledged and refused must be different rendered states"
    );
    assert_ne!(
        unproven_strips[0].1, refused_strips[0].1,
        "the two strips must not read the same"
    );
    assert!(
        refused_strips[0].1.contains(FAILURE_TEXT),
        "a refusal must show the provider's text verbatim; got: {}",
        refused_strips[0].1
    );
    assert_eq!(
        draft(&refused),
        Some(TYPED.to_string()),
        "a refusal must keep the typed text"
    );
}

/// There is no retry affordance anywhere, in any state. The transport is
/// `retry_safe:false`: one tap of a retry button is a second message in
/// someone's live session, and nothing downstream can take it back.
#[gpui::test]
fn no_send_state_offers_a_retry(cx: &mut TestAppContext) {
    for arm in ["proven", "unproven", "refused"] {
        let (rig, vcx) = mount(cx);
        match arm {
            "unproven" => rig.services.arm_unproven(),
            "refused" => rig.services.arm_failure(),
            _ => {}
        }
        compose_and_submit(vcx, &rig, TYPED);

        let labels: Vec<String> = rig
            .bounds
            .all_elements()
            .into_iter()
            .filter_map(|(_, i)| i.displayed_text.as_deref().map(str::to_lowercase))
            .filter(|t| t.contains("retry") || t.contains("resend") || t.contains("try again"))
            .collect();
        assert!(
            labels.is_empty(),
            "the {arm} state must offer no retry affordance; found {labels:?}"
        );
    }
}
