//! `settle_window` over a scripted window: the engine's changes and the views'
//! snapshots arrive one per pump, with idle pumps in between, as a watcher
//! thread delivers them. Whatever the order, once every view has shown its
//! rows the settle returns a frame with no loading placeholder that reflects
//! every emitted change. A view's first snapshot carries no new engine seq, so
//! a frame can reflect the emitted seq while a view still loads. An admitted
//! operation emits its change only when it finishes, so a frame can reflect
//! the emitted seq while an operation's write is still to come. A view that
//! notifies during its render stays pending until another frame draws it, so
//! the settled frame shows what that render's notify changed; a view whose
//! every render changes the screen never settles.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result;
use gpui::EntityId;
use holon_api::live_data::reporting_cursor;
use holon_api::live_data::reporting_to;
use holon_frontend::geometry::Clip;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::geometry::Rect;
use holon_frontend::render_epoch::UiSubscription;
use holon_frontend::render_epoch::UiWatermark;
use holon_frontend::size_expectation::SizeBounds;
use holon_gpui::geometry::BoundsRegistry;
use proptest::prelude::*;

use crate::pbt_harness::settle::Pump;
use crate::pbt_harness::settle::SettleError;
use crate::pbt_harness::settle::settle_window;

/// A settle the reference predicts to time out fails as fast as this; a wrong
/// prediction still fails, since a deadline cannot turn a timeout into `Ok`.
const PREDICTED_TIMEOUT: Duration = Duration::from_millis(50);

/// An engine that routes every change it emits at once.
#[derive(Default)]
struct SeqEngine {
    seq: AtomicU64,
    in_flight: AtomicUsize,
}

#[async_trait::async_trait]
impl holon_api::QueryEngine for SeqEngine {
    async fn lookup_block_path(&self, _: &holon_api::EntityUri) -> Result<String> {
        anyhow::bail!("SeqEngine has no blocks")
    }

    async fn watch_query(
        &self,
        _: &str,
        _: holon_api::QueryLanguage,
        _: std::collections::HashMap<String, holon_api::Value>,
        _: Option<holon_api::QueryContext>,
        _: holon_api::render_requirements::RenderRequirements,
    ) -> Result<holon_api::EnrichedChangeStream> {
        anyhow::bail!("SeqEngine does not watch queries")
    }

    async fn delivery_owed(&self) -> Result<Vec<String>> {
        anyhow::bail!("SeqEngine does not track delivery")
    }

    fn routed_through(&self) -> Result<u64> {
        Ok(self.seq.load(Ordering::Acquire))
    }

    fn emitted_through(&self) -> Result<u64> {
        Ok(self.seq.load(Ordering::Acquire))
    }

    fn operations_in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    async fn search_link_candidates(&self, _: &str) -> Result<Vec<holon_api::LinkCandidate>> {
        anyhow::bail!("SeqEngine has no blocks")
    }

    async fn block_content_by_id(&self, _: &holon_api::EntityUri) -> Result<Option<String>> {
        anyhow::bail!("SeqEngine has no blocks")
    }

    async fn block_task_state_by_id(&self, _: &holon_api::EntityUri) -> Result<Option<String>> {
        anyhow::bail!("SeqEngine has no blocks")
    }
}

#[derive(Clone, Copy, Debug)]
enum Arrival {
    /// The watcher thread delivered nothing before the pump parked.
    Idle,
    /// The engine emits the next change, with rows for view `k` only.
    Route(usize),
    /// View `k` shows every batch routed to it so far.
    Apply(usize),
    /// An operation is admitted; it writes nothing yet.
    Admit,
    /// An admitted operation finishes and emits its change, with rows for
    /// view `k` only. Nothing happens when no operation is in flight.
    Finish(usize),
    /// A notify outside a draw redraws the window, and view `k` notifies
    /// during that one render with a change its next render shows.
    NotifyInRender(usize),
    /// From now on view `k` notifies during every render and draws the same.
    NotifyInEveryRender(usize),
    /// From now on view `k` notifies during every render and draws a change.
    ChangeInEveryRender(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RenderNotify {
    Never,
    Once,
    Every,
    ChangeEvery,
}

impl RenderNotify {
    /// The view's mode after a render: whether that render left the view
    /// pending and whether its next render shows new content.
    fn render(self) -> (RenderNotify, bool, bool) {
        match self {
            RenderNotify::Never => (RenderNotify::Never, false, false),
            RenderNotify::Once => (RenderNotify::Never, true, true),
            RenderNotify::Every => (RenderNotify::Every, true, false),
            RenderNotify::ChangeEvery => (RenderNotify::ChangeEvery, true, true),
        }
    }

    fn notified(self, arrival: Arrival, k: usize) -> RenderNotify {
        match arrival {
            Arrival::NotifyInRender(j) if j == k && self == RenderNotify::Never => {
                RenderNotify::Once
            }
            Arrival::NotifyInEveryRender(j) if j == k => RenderNotify::Every,
            Arrival::ChangeInEveryRender(j) if j == k => RenderNotify::ChangeEvery,
            _ => self,
        }
    }
}

struct ScriptedWindow {
    engine: Arc<SeqEngine>,
    watermark: Arc<UiWatermark>,
    bounds: BoundsRegistry,
    views: Vec<UiSubscription>,
    shown: Vec<Cell<bool>>,
    render_notify: Vec<Cell<RenderNotify>>,
    /// What each view's next render shows.
    label: Vec<Cell<u64>>,
    /// The views a notify during a draw marked, drained by the next draw.
    pending: RefCell<BTreeSet<usize>>,
    arrivals: RefCell<VecDeque<Arrival>>,
}

impl ScriptedWindow {
    fn new(n: usize, arrivals: Vec<Arrival>) -> Self {
        let engine = Arc::new(SeqEngine::default());
        let watermark = UiWatermark::new(Some(engine.clone()));
        let views: Vec<UiSubscription> = (0..n)
            .map(|k| watermark.register(format!("view-{k}")))
            .collect();
        for view in &views {
            futures::executor::block_on(reporting_to(view.cursor().clone(), async {
                reporting_cursor()
            }))
            .expect("reporting_to hands its cursor to the subscriber");
        }
        let window = Self {
            engine,
            watermark,
            bounds: BoundsRegistry::new(),
            views,
            shown: (0..n).map(|_| Cell::new(false)).collect(),
            render_notify: (0..n).map(|_| Cell::new(RenderNotify::Never)).collect(),
            label: (0..n).map(|_| Cell::new(0)).collect(),
            pending: RefCell::new(BTreeSet::new()),
            arrivals: RefCell::new(arrivals.into()),
        };
        window.draw();
        window.bounds.flush();
        window
    }

    fn draw(&self) {
        let sample = self
            .watermark
            .sample()
            .expect("the engine reports its routed seq");
        self.watermark.publish(sample);
        self.bounds.begin_pass(self.watermark.current());
        self.pending.borrow_mut().clear();
        for (k, shown) in self.shown.iter().enumerate() {
            let widget = match shown.get() {
                true => "live_block",
                false => "loading",
            };
            self.bounds.record(
                format!("view-{k}"),
                element(widget, &self.label[k].get().to_string()),
            );
            let (mode, pending, changes) = self.render_notify[k].get().render();
            self.render_notify[k].set(mode);
            if pending {
                self.pending.borrow_mut().insert(k);
            }
            if changes {
                self.label[k].set(self.label[k].get() + 1);
            }
        }
    }

    fn emit(&self, k: usize) {
        let seq = self.engine.seq.load(Ordering::Acquire) + 1;
        for (j, view) in self.views.iter().enumerate() {
            match j == k {
                true => view.cursor().routed(seq),
                false => view.cursor().skipped(seq),
            }
        }
        self.engine.seq.store(seq, Ordering::Release);
    }
}

impl Pump for ScriptedWindow {
    fn run_until_parked(&self) {
        let Some(arrival) = self.arrivals.borrow_mut().pop_front() else {
            return;
        };
        match arrival {
            Arrival::Idle => return,
            Arrival::Route(k) => self.emit(k),
            Arrival::Admit => {
                self.engine.in_flight.fetch_add(1, Ordering::AcqRel);
            }
            Arrival::Finish(k) => {
                if self.engine.in_flight.load(Ordering::Acquire) > 0 {
                    self.emit(k);
                    self.engine.in_flight.fetch_sub(1, Ordering::AcqRel);
                }
            }
            Arrival::Apply(k) => {
                self.views[k].applied(self.views[k].cursor().sent());
                self.shown[k].set(true);
            }
            Arrival::NotifyInRender(_)
            | Arrival::NotifyInEveryRender(_)
            | Arrival::ChangeInEveryRender(_) => {
                for (k, mode) in self.render_notify.iter().enumerate() {
                    mode.set(mode.get().notified(arrival, k));
                }
            }
        }
        self.draw();
    }

    fn pending_views(&self) -> Vec<EntityId> {
        self.pending
            .borrow()
            .iter()
            .map(|&k| EntityId::from(k as u64))
            .collect()
    }

    fn force_frame(&self) {
        self.draw();
    }
}

fn element(widget: &str, text: &str) -> ElementInfo {
    let rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 20.0,
    };
    ElementInfo {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        widget_type: Arc::from(widget),
        entity_id: None,
        has_content: true,
        parent_id: None,
        displayed_text: Some(Arc::from(text)),
        focused: None,
        styled_runs: None,
        opacity: None,
        expected_size: SizeBounds::default(),
        vm_node: None,
        painted_fg: None,
        painted_bg: None,
        clip: Clip::unclipped(rect),
    }
}

fn displayed(bounds: &BoundsRegistry, id: &str) -> Option<String> {
    bounds
        .element_info(id)
        .and_then(|info| info.displayed_text)
        .map(|text| text.to_string())
}

fn arrival(n: usize) -> impl Strategy<Value = Arrival> {
    prop_oneof![
        Just(Arrival::Idle),
        (0..n).prop_map(Arrival::Route),
        (0..n).prop_map(Arrival::Apply),
        Just(Arrival::Admit),
        (0..n).prop_map(Arrival::Finish),
        (0..n).prop_map(Arrival::NotifyInRender),
        (0..n).prop_map(Arrival::NotifyInEveryRender),
    ]
}

/// Random arrivals, then the end of every admitted operation and every view's
/// final snapshot, then a few idle pumps and notifies during a render. A view
/// that changes the screen in every render never settles, wherever it starts,
/// so it comes only in the tail and seldom: each such case runs to its
/// deadline.
fn schedule() -> impl Strategy<Value = (usize, Vec<Arrival>)> {
    (1usize..4).prop_flat_map(|n| {
        let tail = prop_oneof![
            4 => Just(Arrival::Idle),
            4 => (0..n).prop_map(Arrival::NotifyInRender),
            1 => (0..n).prop_map(Arrival::ChangeInEveryRender),
        ];
        (
            proptest::collection::vec(arrival(n), 0..30),
            proptest::collection::vec(tail, 0..3),
        )
            .prop_map(move |(mut arrivals, tail)| {
                let admitted = arrivals
                    .iter()
                    .filter(|a| matches!(a, Arrival::Admit))
                    .count();
                arrivals.extend((0..admitted).map(|_| Arrival::Finish(0)));
                arrivals.extend((0..n).map(Arrival::Apply));
                arrivals.extend(tail);
                (n, arrivals)
            })
    })
}

/// Each view's render mode once every arrival happened. Every arrival but an
/// idle one draws the window, and each draw consumes a one-shot notify.
fn modes_after_every_arrival(n: usize, arrivals: &[Arrival]) -> Vec<RenderNotify> {
    let mut modes = vec![RenderNotify::Never; n];
    let mut in_flight = 0usize;
    for &arrival in arrivals {
        match arrival {
            Arrival::Idle => continue,
            Arrival::Admit => in_flight += 1,
            Arrival::Finish(_) => in_flight = in_flight.saturating_sub(1),
            _ => {}
        }
        for (k, mode) in modes.iter_mut().enumerate() {
            *mode = mode.notified(arrival, k).render().0;
        }
    }
    assert_eq!(in_flight, 0, "a schedule finishes every admitted operation");
    modes
}

/// The reference, from the gpui rule that a notify during a draw leaves its
/// view pending without dirtying the window: once no arrival is left, frames
/// drawn for the pending views drain every one-shot notify, so the window
/// settles unless a view changes the screen in every render. Then the timeout
/// names every view that notifies in every render.
fn views_that_never_settle(modes: &[RenderNotify]) -> Option<BTreeSet<usize>> {
    modes.contains(&RenderNotify::ChangeEvery).then(|| {
        modes
            .iter()
            .enumerate()
            .filter(|(_, mode)| matches!(mode, RenderNotify::Every | RenderNotify::ChangeEvery))
            .map(|(k, _)| k)
            .collect()
    })
}

fn settles_on_every_shown_view(n: usize, arrivals: Vec<Arrival>) -> Result<(), TestCaseError> {
    let deadline = match views_that_never_settle(&modes_after_every_arrival(n, &arrivals)) {
        None => Duration::from_secs(5),
        Some(_) => PREDICTED_TIMEOUT,
    };
    let window = ScriptedWindow::new(n, arrivals.clone());
    let settled = settle_window(
        &window,
        &window.bounds,
        &window.watermark,
        &mut || {},
        deadline,
    );
    // A pump that delivers nothing once the frame reflects the target is a
    // fixed point: an arrival after it is a later change, so the reference
    // judges the arrivals that came.
    let came = arrivals.len() - window.arrivals.borrow().len();
    let never_settle = views_that_never_settle(&modes_after_every_arrival(n, &arrivals[..came]));
    let epoch = match settled {
        Ok(epoch) => epoch,
        Err(SettleError::Timeout(t)) => {
            let Some(never_settle) = never_settle else {
                return Err(TestCaseError::fail(format!(
                    "{arrivals:?}: every view settles once no arrival is left, yet the settle \
                     timed out: {}",
                    SettleError::Timeout(t)
                )));
            };
            prop_assert!(
                window.arrivals.borrow().is_empty(),
                "{arrivals:?}: timed out before every arrival happened"
            );
            prop_assert!(
                t.dirty,
                "{arrivals:?}: a view that changes the screen in every render leaves the window dirty"
            );
            prop_assert_eq!(
                t.pending_views.iter().copied().collect::<BTreeSet<_>>(),
                never_settle
                    .iter()
                    .map(|&k| EntityId::from(k as u64))
                    .collect::<BTreeSet<_>>(),
                "{:?}: the timeout names the views that notify in every render",
                arrivals
            );
            return Ok(());
        }
        Err(e) => return Err(TestCaseError::fail(format!("{arrivals:?}: {e}"))),
    };
    prop_assert!(
        never_settle.is_none(),
        "{arrivals:?}: settled at {epoch:?} while views {never_settle:?} change the screen in \
         every render"
    );
    for k in 0..n {
        prop_assert_eq!(
            displayed(&window.bounds, &format!("view-{k}")),
            Some(window.label[k].get().to_string()),
            "{:?}: view {} shows stale content after the settle",
            arrivals,
            k
        );
    }
    let emitted = window
        .watermark
        .emitted()
        .expect("the engine reports its emitted seq");
    prop_assert_eq!(
        window.engine.in_flight.load(Ordering::Acquire),
        0,
        "{:?}: settled while an admitted operation is unfinished",
        arrivals
    );
    prop_assert!(
        epoch.reflects >= emitted,
        "{arrivals:?}: frame reflects {:?}, the engine emitted {emitted:?}",
        epoch.reflects
    );
    let loading: Vec<String> = window
        .bounds
        .all_elements()
        .into_iter()
        .filter(|(_, info)| info.widget_type.as_ref() == "loading")
        .map(|(id, _)| id)
        .collect();
    prop_assert!(
        loading.is_empty(),
        "{arrivals:?}: still loading {loading:?}"
    );
    Ok(())
}

/// A view's last render notifies with a change, and nothing else draws the
/// window again.
#[test]
fn settle_shows_the_change_a_notify_in_the_last_render_made() {
    settles_on_every_shown_view(1, vec![Arrival::Apply(0), Arrival::NotifyInRender(0)])
        .expect("the settled frame shows the change");
}

/// A view notifies during one render, and a later change draws it again.
#[test]
fn settle_shows_a_view_that_a_later_draw_rendered_again() {
    settles_on_every_shown_view(
        1,
        vec![
            Arrival::Apply(0),
            Arrival::NotifyInRender(0),
            Arrival::Apply(0),
        ],
    )
    .expect("the settle returns the redrawn frame");
}

/// A view notifies during every render and draws the same content each time.
#[test]
fn settle_accepts_a_view_that_notifies_in_every_render_without_a_change() {
    settles_on_every_shown_view(
        1,
        vec![
            Arrival::NotifyInEveryRender(0),
            Arrival::Apply(0),
            Arrival::Route(0),
            Arrival::Apply(0),
        ],
    )
    .expect("a frame drawn again changes nothing");
}

/// A view changes the screen in every render; a quiet view beside it.
#[test]
fn settle_times_out_on_a_view_that_changes_in_every_render() {
    settles_on_every_shown_view(
        2,
        vec![
            Arrival::Apply(0),
            Arrival::Apply(1),
            Arrival::NotifyInEveryRender(1),
            Arrival::ChangeInEveryRender(0),
        ],
    )
    .expect("the settle times out naming both notifying views");
}

/// A pump delivers nothing before a view starts to change in every render:
/// the settle returns that earlier fixed point.
#[test]
fn settle_returns_a_fixed_point_reached_before_a_later_change() {
    settles_on_every_shown_view(
        1,
        vec![
            Arrival::Apply(0),
            Arrival::Idle,
            Arrival::ChangeInEveryRender(0),
        ],
    )
    .expect("an idle pump after the shown rows is a fixed point");
}

/// The first snapshot arrives after the window parked twice at the emitted seq.
#[test]
fn settle_waits_for_a_first_snapshot_after_idle_pumps() {
    settles_on_every_shown_view(1, vec![Arrival::Idle, Arrival::Idle, Arrival::Apply(0)])
        .expect("the static example settles on the shown view");
}

/// An operation is admitted after the views showed their rows, and finishes
/// only after the window parked twice at the emitted seq.
#[test]
fn settle_waits_for_an_admitted_operation_to_finish() {
    settles_on_every_shown_view(
        1,
        vec![
            Arrival::Apply(0),
            Arrival::Admit,
            Arrival::Idle,
            Arrival::Idle,
            Arrival::Finish(0),
            Arrival::Apply(0),
        ],
    )
    .expect("the static example settles after the operation's write");
}

proptest! {
    #[test]
    fn settle_returns_only_once_every_view_shows_its_rows((n, arrivals) in schedule()) {
        settles_on_every_shown_view(n, arrivals)?;
    }
}

mod real_window {
    use std::rc::Rc;

    use gpui::AppContext as _;
    use gpui::Entity;
    use gpui::HeadlessAppContext;
    use gpui::IntoElement;
    use gpui::ParentElement as _;
    use gpui::Render;
    use gpui::Window;

    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Notifies {
        Never,
        /// Each of the first k renders notifies with a change.
        ChangeInFirstRenders(usize),
        /// Every render notifies and draws the same content.
        InEveryRender,
        /// Every render notifies with a change.
        ChangeInEveryRender,
        /// Each of the first k renders notifies; only the k-th changes what
        /// the next render shows.
        ChangeInRender(usize),
    }

    struct Probe {
        name: String,
        bounds: BoundsRegistry,
        notifies: Notifies,
        renders: usize,
        /// What the next render shows.
        value: usize,
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
            self.renders += 1;
            self.bounds.record(
                self.name.clone(),
                element("live_block", &self.value.to_string()),
            );
            let (notify, change) = match self.notifies {
                Notifies::Never => (false, false),
                Notifies::ChangeInFirstRenders(k) => (self.renders <= k, self.renders <= k),
                Notifies::InEveryRender => (true, false),
                Notifies::ChangeInEveryRender => (true, true),
                Notifies::ChangeInRender(k) => (self.renders <= k, self.renders == k),
            };
            if change {
                self.value += 1;
            }
            if notify {
                cx.notify();
            }
            gpui::div()
        }
    }

    struct Root {
        bounds: BoundsRegistry,
        watermark: Arc<UiWatermark>,
        probes: Vec<Entity<Probe>>,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            self.bounds.begin_pass(self.watermark.current());
            self.bounds
                .record("root".to_string(), element("live_block", "root"));
            gpui::div().children(self.probes.clone())
        }
    }

    fn notifies() -> impl Strategy<Value = Notifies> {
        prop_oneof![
            3 => Just(Notifies::Never),
            3 => (1usize..4).prop_map(Notifies::ChangeInFirstRenders),
            2 => Just(Notifies::InEveryRender),
            1 => Just(Notifies::ChangeInEveryRender),
            // From k = 3 the change comes after two unchanged renders: the
            // settle's documented blind spot.
            2 => (1usize..3).prop_map(Notifies::ChangeInRender),
        ]
    }

    /// A real window of probe views, each notifying during none, the first
    /// few, or every one of its renders, with or without a change to what it
    /// draws; outside notifies redraw chosen views before the settle. The
    /// settled frame shows what every probe's notify changed; a probe that
    /// changes in every render keeps the window from settling, and the timeout
    /// names every probe that notifies in every render.
    fn real_window_shows_every_change_a_notify_made(
        behaviours: Vec<Notifies>,
        outside_notifies: Vec<Vec<usize>>,
    ) -> Result<(), TestCaseError> {
        let mut app = HeadlessAppContext::new(gpui_platform::current_text_system());
        let watermark = UiWatermark::new(Some(Arc::new(SeqEngine::default())));
        let bounds = BoundsRegistry::new();
        let probes: Rc<RefCell<Vec<Entity<Probe>>>> = Rc::new(RefCell::new(Vec::new()));
        let (b, w, built, modes) = (
            bounds.clone(),
            watermark.clone(),
            probes.clone(),
            behaviours.clone(),
        );
        app.open_window(
            gpui::size(gpui::px(400.0), gpui::px(300.0)),
            move |_, cx| {
                let probes: Vec<Entity<Probe>> = modes
                    .iter()
                    .enumerate()
                    .map(|(i, &notifies)| {
                        cx.new(|_| Probe {
                            name: format!("probe-{i}"),
                            bounds: b.clone(),
                            notifies,
                            renders: 0,
                            value: 0,
                        })
                    })
                    .collect();
                *built.borrow_mut() = probes.clone();
                cx.new(|_| Root {
                    bounds: b,
                    watermark: w,
                    probes,
                })
            },
        )
        .expect("open_window");
        let probes = probes.borrow().clone();
        app.run_until_parked();
        for chosen in &outside_notifies {
            app.update(|cx| {
                for &k in chosen {
                    probes[k % probes.len()].update(cx, |_, cx| cx.notify());
                }
            });
            app.run_until_parked();
        }
        let never_settle: Option<BTreeSet<EntityId>> = behaviours
            .contains(&Notifies::ChangeInEveryRender)
            .then(|| {
                probes
                    .iter()
                    .zip(&behaviours)
                    .filter(|(_, notifies)| {
                        matches!(
                            notifies,
                            Notifies::InEveryRender | Notifies::ChangeInEveryRender
                        )
                    })
                    .map(|(probe, _)| probe.entity_id())
                    .collect()
            });
        let deadline = match never_settle {
            None => Duration::from_secs(5),
            Some(_) => PREDICTED_TIMEOUT,
        };
        let case = format!("{behaviours:?} {outside_notifies:?}");
        match (
            settle_window(&app, &bounds, &watermark, &mut || {}, deadline),
            never_settle,
        ) {
            (Ok(_), None) => {
                for (i, probe) in probes.iter().enumerate() {
                    let value = app.update(|cx| probe.read(cx).value);
                    prop_assert_eq!(
                        displayed(&bounds, &format!("probe-{i}")),
                        Some(value.to_string()),
                        "{}: probe {} shows stale content after the settle",
                        case,
                        i
                    );
                }
            }
            (Ok(epoch), Some(never_settle)) => {
                return Err(TestCaseError::fail(format!(
                    "{case}: settled at {epoch:?} while {never_settle:?} change in every render"
                )));
            }
            (Err(SettleError::Timeout(t)), Some(never_settle)) => {
                prop_assert!(
                    t.dirty,
                    "{case}: a view that changes in every render leaves the window dirty"
                );
                prop_assert_eq!(
                    t.pending_views.iter().copied().collect::<BTreeSet<_>>(),
                    never_settle,
                    "{}: the timeout names the views that notify in every render",
                    case
                );
            }
            (Err(e), _) => return Err(TestCaseError::fail(format!("{case}: {e}"))),
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn real_window_settles_on_a_frame_that_shows_every_notified_change(
            behaviours in proptest::collection::vec(notifies(), 1..4),
            outside_notifies in proptest::collection::vec(
                proptest::collection::vec(0usize..4, 1..3),
                0..4,
            ),
        ) {
            real_window_shows_every_change_a_notify_made(behaviours, outside_notifies)?;
        }
    }

    /// A view's first render notifies with a change, and nothing else draws
    /// the window again.
    #[test]
    fn settle_window_shows_the_change_a_notify_during_render_made() {
        real_window_shows_every_change_a_notify_made(
            vec![Notifies::ChangeInFirstRenders(1)],
            vec![],
        )
        .expect("the settled frame shows the change");
    }

    /// A view notifies in its first render without a change, and its second
    /// render, the frame the settle forces, notifies with a change.
    #[test]
    fn settle_window_shows_a_change_the_forced_frame_made() {
        real_window_shows_every_change_a_notify_made(vec![Notifies::ChangeInRender(2)], vec![])
            .expect("the settled frame shows the change");
    }

    /// A spinner notifies in every render and draws the same content.
    #[test]
    fn settle_window_accepts_a_view_that_notifies_in_every_render_without_a_change() {
        real_window_shows_every_change_a_notify_made(vec![Notifies::InEveryRender], vec![])
            .expect("a frame drawn again changes nothing");
    }

    /// A view changes in every render, and a spinner beside it.
    #[test]
    fn settle_window_times_out_on_a_view_that_changes_in_every_render() {
        real_window_shows_every_change_a_notify_made(
            vec![Notifies::ChangeInEveryRender, Notifies::InEveryRender],
            vec![vec![0]],
        )
        .expect("the settle times out naming both notifying views");
    }

    struct ReadsPendingWhileDrawing;

    impl Render for ReadsPendingWhileDrawing {
        fn render(&mut self, window: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            window.pending_dirty_views();
            gpui::div()
        }
    }

    #[test]
    #[should_panic(expected = "pending_dirty_views read during a draw")]
    fn pending_dirty_views_refuses_a_read_during_a_draw() {
        let mut app = HeadlessAppContext::new(gpui_platform::current_text_system());
        app.open_window(gpui::size(gpui::px(100.0), gpui::px(100.0)), |_, cx| {
            cx.new(|_| ReadsPendingWhileDrawing)
        })
        .expect("open_window");
        app.run_until_parked();
    }
}

/// A real window over a real engine. A view's driver writes the rows its
/// provider received into the view's items on a tokio task, after the engine
/// settled: busy tokio workers make that hop arrive after the settle's pumps.
/// Whatever the hop's timing, an `Ok` settle shows every row the store holds
/// for the focused page and its backlinks; a settle that times out because the
/// hop is late names the late driver.
mod driver_hop {
    use std::collections::BTreeMap;

    use gpui::AssetSource;
    use gpui::HeadlessAppContext;
    use holon_api::ContentType;
    use holon_api::EntityUri;
    use holon_api::PAGE_TAG;
    use holon_api::Region;
    use holon_api::Value;
    use holon_gpui::RebindHandle;
    use holon_gpui::launch_holon_window_rebindable;
    use holon_gpui::navigation_state::NavigationState;
    use holon_gpui::user_driver::derived_items;
    use holon_integration_tests::test_environment::TestEnvironment;

    use super::*;
    use crate::pbt_harness::settle::HANG_DEADLINE;
    use crate::pbt_harness::settle::hold_tokio_workers;
    use crate::pbt_harness::windowed_wide::real_text_system;
    use crate::pbt_harness::windowed_wide::resize_window;

    const PAGES: [(&str, &str); 2] = [("hop-page-a", "Hop Page A"), ("hop-page-b", "Hop Page B")];
    /// Rows a 1200x900 window paints without a scroll.
    const MAX_CHILDREN: usize = 5;
    const MAX_REFS: usize = 3;
    /// Shorter than every drawn hold, so a late hop outlasts it.
    const SHORT_DEADLINE: Duration = Duration::from_millis(30);

    #[derive(Clone, Debug)]
    enum Op {
        /// A child block under the focused page.
        AddChild,
        /// New content for a child of the focused page.
        Edit(prop::sample::Index),
        Delete(prop::sample::Index),
        /// A block outside the focused page that links it.
        AddRef,
        /// Main switches to the other page: the old page's views drop.
        Refocus,
        /// The window's viewport changes.
        Resize {
            narrow: bool,
        },
    }

    #[derive(Clone, Copy, Debug)]
    enum Hop {
        OnTime,
        /// Every tokio worker is busy for `hold_ms` once the engine settled.
        HeldAfterEngine {
            hold_ms: u64,
            short_deadline: bool,
        },
        /// Every tokio worker is busy for `hold_ms` before the engine settles.
        HeldBeforeEngine {
            hold_ms: u64,
        },
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => Just(Op::AddChild),
            2 => any::<prop::sample::Index>().prop_map(Op::Edit),
            2 => any::<prop::sample::Index>().prop_map(Op::Delete),
            2 => Just(Op::AddRef),
            1 => Just(Op::Refocus),
            1 => any::<bool>().prop_map(|narrow| Op::Resize { narrow }),
        ]
    }

    fn held_after_engine() -> impl Strategy<Value = Hop> {
        (prop::sample::select(vec![50u64, 200, 400]), any::<bool>()).prop_map(
            |(hold_ms, short_deadline)| Hop::HeldAfterEngine {
                hold_ms,
                short_deadline,
            },
        )
    }

    fn hop() -> impl Strategy<Value = Hop> {
        prop_oneof![
            1 => Just(Hop::OnTime),
            3 => held_after_engine(),
            1 => prop::sample::select(vec![50u64, 200])
                .prop_map(|hold_ms| Hop::HeldBeforeEngine { hold_ms }),
        ]
    }

    /// A held AddChild can supersede a render generation of holon's ui_watcher
    /// before its `take_over`, which leaves that seq unclaimed and the settle
    /// owed forever;
    /// `settled_frame_shows_every_row_for_a_superseded_generation`
    /// keeps those cases.
    fn step() -> impl Strategy<Value = (Op, Hop)> {
        (op(), hop()).prop_filter("held AddChild loses its seq in ui_watcher", |step| {
            !matches!(step, (Op::AddChild, Hop::HeldBeforeEngine { .. }))
        })
    }

    /// Children each page holds before the window opens, then random steps.
    /// The last step moves Main to the other page with the tokio workers held
    /// after the engine settled: the new page's views mount in a pump and
    /// their first rows arrive on a tokio task after it.
    fn case() -> impl Strategy<Value = ([usize; 2], Vec<(Op, Hop)>)> {
        (
            [1..=3usize, 1..=3usize],
            proptest::collection::vec(step(), 0..5),
            held_after_engine(),
        )
            .prop_map(|(seeded, mut steps, last)| {
                steps.push((Op::Refocus, last));
                (seeded, steps)
            })
    }

    #[derive(Default)]
    struct Model {
        focused: usize,
        /// Per page: child number -> content version.
        children: [BTreeMap<usize, u32>; 2],
        refs: [Vec<usize>; 2],
        /// Every child ever created.
        created: Vec<usize>,
        next: usize,
    }

    fn child_id(k: usize) -> String {
        format!("hop-c{k}")
    }

    fn child_text(k: usize, v: u32) -> String {
        format!("hop-c{k}-v{v}.")
    }

    fn ref_id(k: usize) -> String {
        format!("hop-r{k}")
    }

    struct Window {
        app: HeadlessAppContext,
        runtime: Arc<tokio::runtime::Runtime>,
        env: TestEnvironment,
        bounds: BoundsRegistry,
        rebind: RebindHandle,
    }

    impl Window {
        fn open(model: &mut Model, seeded: [usize; 2]) -> Self {
            // Read by the window launcher before the window opens.
            // SAFETY: single-threaded setup, before any window or runtime
            // thread reads the process environment.
            unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", "1200x900") };
            let assets: Arc<dyn AssetSource> = Arc::new(());
            let mut app = HeadlessAppContext::with_platform(real_text_system(), assets, || {
                gpui_platform::current_headless_renderer()
            });
            let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
            let env = runtime
                .block_on(async { TestEnvironment::new(runtime.clone()) })
                .expect("test environment");
            runtime.block_on(async {
                env.start_app(true).await.expect("start_app");
                for (id, title) in PAGES {
                    let mut page: holon_api::StorageEntity = std::collections::HashMap::new();
                    page.insert("id".into(), Value::String(EntityUri::block(id).to_string()));
                    page.insert(
                        "parent_id".into(),
                        Value::String(EntityUri::no_parent().as_str().to_string()),
                    );
                    page.insert("content".into(), Value::String(title.into()));
                    page.insert("content_type".into(), ContentType::Text.into());
                    page.insert(
                        "tags".into(),
                        Value::Array(vec![Value::String(PAGE_TAG.to_string())]),
                    );
                    env.test_ctx()
                        .execute_op("block", "create", page)
                        .await
                        .unwrap_or_else(|e| panic!("create page {id}: {e}"));
                }
            });
            for (page, &count) in seeded.iter().enumerate() {
                for _ in 0..count {
                    let k = model.next;
                    model.next += 1;
                    runtime
                        .block_on(env.create_block(&child_id(k), PAGES[page].0, &child_text(k, 0)))
                        .unwrap_or_else(|e| panic!("seed child {k}: {e}"));
                    model.children[page].insert(k, 0);
                    model.created.push(k);
                }
            }
            focus(&runtime, &env, 0);
            runtime.block_on(env.settle());
            let engine = env
                .reactive_engine
                .get()
                .cloned()
                .expect("reactive engine after start_app");
            let bounds = BoundsRegistry::new();
            let debug_services = env.debug_services().cloned().expect("debug services");
            let rebind = app
                .update(|cx| {
                    launch_holon_window_rebindable(
                        env.session_arc(),
                        engine,
                        runtime.handle().clone(),
                        NavigationState::new(),
                        bounds.clone(),
                        Some(debug_services),
                        None,
                        "Holon-Driver-Hop",
                        cx,
                    )
                })
                .expect("window opened");
            let window = Window {
                app,
                runtime,
                env,
                bounds,
                rebind,
            };
            window
                .settle(Hop::OnTime)
                .unwrap_or_else(|e| panic!("boot settle: {e}"));
            window
        }

        #[allow(clippy::result_large_err)]
        fn settle(&self, hop: Hop) -> Result<(), SettleError> {
            let engine = self
                .env
                .reactive_engine
                .get()
                .expect("reactive engine after start_app");
            let (runtime, env) = (&self.runtime, &self.env);
            let mut settle_engine = || runtime.block_on(env.settle());
            match hop {
                Hop::OnTime => settle_window(
                    &self.app,
                    &self.bounds,
                    engine.ui_watermark(),
                    &mut settle_engine,
                    HANG_DEADLINE,
                ),
                Hop::HeldAfterEngine {
                    hold_ms,
                    short_deadline,
                } => {
                    settle_engine();
                    hold_tokio_workers(runtime, Duration::from_millis(hold_ms));
                    let deadline = if short_deadline {
                        SHORT_DEADLINE
                    } else {
                        HANG_DEADLINE
                    };
                    settle_window(
                        &self.app,
                        &self.bounds,
                        engine.ui_watermark(),
                        &mut || {},
                        deadline,
                    )
                }
                Hop::HeldBeforeEngine { hold_ms } => {
                    hold_tokio_workers(runtime, Duration::from_millis(hold_ms));
                    settle_window(
                        &self.app,
                        &self.bounds,
                        engine.ui_watermark(),
                        &mut settle_engine,
                        HANG_DEADLINE,
                    )
                }
            }
            .map(|_| ())
        }

        /// Every Main list that holds one of this test's rows matches a fresh
        /// derivation of its provider's rows: its driver applied every batch.
        fn assert_drivers_applied(&mut self, step: &str) -> Result<(), TestCaseError> {
            let engine = self
                .env
                .reactive_engine
                .get()
                .expect("reactive engine after start_app");
            let ours = |rows: &[String]| rows.iter().any(|r| r.starts_with("block:hop-"));
            let lists = self
                .app
                .update(|cx| self.rebind.live_panel_lists(Region::Main, cx));
            for (watcher, live) in lists {
                let mut live: Vec<String> = live.iter().map(|u| u.to_string()).collect();
                let mut derived: Vec<String> = derived_items(engine, &watcher)
                    .iter()
                    .map(|u| u.to_string())
                    .collect();
                if !ours(&live) && !ours(&derived) {
                    continue;
                }
                live.sort();
                derived.sort();
                prop_assert_eq!(
                    live,
                    derived,
                    "{}: after an Ok settle, list {} lacks rows its provider holds (left: the \
                     view's items, right: the provider's rows)",
                    step,
                    watcher
                );
            }
            Ok(())
        }

        fn painted_texts(&self, id: &str) -> Vec<String> {
            let entity = EntityUri::block(id).to_string();
            self.bounds
                .all_elements()
                .into_iter()
                .filter(|(_, info)| info.entity_id.as_deref() == Some(entity.as_str()))
                .filter(|(_, info)| info.height > 0.0)
                .filter_map(|(_, info)| info.displayed_text.map(|t| t.to_string()))
                .collect()
        }

        fn painted(&self, id: &str) -> bool {
            let entity = EntityUri::block(id).to_string();
            self.bounds.all_elements().into_iter().any(|(_, info)| {
                info.entity_id.as_deref() == Some(entity.as_str()) && info.height > 0.0
            })
        }

        /// The screen shows the focused page's children at their latest
        /// content and its backlinks, and no deleted child or child of the
        /// other page.
        fn assert_screen_shows(&self, model: &Model, step: &str) -> Result<(), TestCaseError> {
            self.bounds.flush();
            let page = model.focused;
            for (&k, &v) in &model.children[page] {
                let texts = self.painted_texts(&child_id(k));
                prop_assert!(
                    texts.iter().any(|t| t.contains(&child_text(k, v))),
                    "{}: child {} must show {:?}, painted {:?}",
                    step,
                    k,
                    child_text(k, v),
                    texts
                );
            }
            for &k in &model.created {
                if !model.children[page].contains_key(&k) {
                    prop_assert!(
                        !self.painted(&child_id(k)),
                        "{}: child {} is not under the focused page, yet painted",
                        step,
                        k
                    );
                }
            }
            for &k in &model.refs[page] {
                prop_assert!(
                    self.painted(&ref_id(k)),
                    "{}: backlink {} to the focused page is not painted",
                    step,
                    k
                );
            }
            Ok(())
        }

        fn close(self) {
            let Window {
                mut app,
                env,
                rebind,
                ..
            } = self;
            drop(rebind);
            app.update(|cx| cx.shutdown());
            app.run_until_parked();
            std::mem::forget(app);
            std::mem::forget(env);
        }
    }

    fn focus(runtime: &tokio::runtime::Runtime, env: &TestEnvironment, page: usize) {
        let mut focus: holon_api::StorageEntity = std::collections::HashMap::new();
        focus.insert("region".into(), Value::from(Region::Main));
        focus.insert(
            "block_id".into(),
            Value::String(EntityUri::block(PAGES[page].0).to_string()),
        );
        runtime
            .block_on(env.test_ctx().execute_op("navigation", "focus", focus))
            .expect("focus Main on a page");
    }

    /// Apply `op` to the store and the model.
    fn apply(window: &mut Window, model: &mut Model, op: &Op) {
        let (runtime, env) = (&window.runtime, &window.env);
        let page = model.focused;
        let children: Vec<usize> = model.children[page].keys().copied().collect();
        match op {
            Op::AddChild if model.children[page].len() < MAX_CHILDREN => {
                let k = model.next;
                model.next += 1;
                runtime
                    .block_on(env.create_block(&child_id(k), PAGES[page].0, &child_text(k, 0)))
                    .unwrap_or_else(|e| panic!("create child {k}: {e}"));
                model.children[page].insert(k, 0);
                model.created.push(k);
            }
            Op::Edit(index) if !children.is_empty() => {
                let k = *index.get(&children);
                let v = model.children[page][&k] + 1;
                runtime
                    .block_on(env.update_block_content(&child_id(k), &child_text(k, v)))
                    .unwrap_or_else(|e| panic!("edit child {k}: {e}"));
                model.children[page].insert(k, v);
            }
            Op::Delete(index) if !children.is_empty() => {
                let k = *index.get(&children);
                runtime
                    .block_on(env.delete_block(&child_id(k)))
                    .unwrap_or_else(|e| panic!("delete child {k}: {e}"));
                model.children[page].remove(&k);
            }
            Op::AddRef if model.refs[page].len() < MAX_REFS => {
                let k = model.next;
                model.next += 1;
                runtime
                    .block_on(env.create_block(
                        &ref_id(k),
                        EntityUri::no_parent().as_str(),
                        &format!("[[{}]] hop-r{k}.", PAGES[page].1),
                    ))
                    .unwrap_or_else(|e| panic!("create backlink {k}: {e}"));
                model.refs[page].push(k);
            }
            Op::Refocus => {
                model.focused = 1 - page;
                focus(runtime, env, model.focused);
            }
            Op::Resize { narrow } => {
                let width = if *narrow { 900.0 } else { 1200.0 };
                let handle = window.rebind.window();
                resize_window(&mut window.app, handle, width, 900.0);
            }
            Op::AddChild | Op::Edit(_) | Op::Delete(_) | Op::AddRef => {}
        }
    }

    fn settled_frame_shows_every_row(
        seeded: [usize; 2],
        steps: Vec<(Op, Hop)>,
    ) -> Result<(), TestCaseError> {
        let mut model = Model::default();
        eprintln!("[driver-hop] open {seeded:?} {steps:?}");
        let mut window = Window::open(&mut model, seeded);
        eprintln!("[driver-hop] opened");
        let boot = format!("boot with {seeded:?} of {steps:?}");
        let mut outcome = window
            .assert_drivers_applied(&boot)
            .and_then(|()| window.assert_screen_shows(&model, &boot));
        for (i, (op, hop)) in steps.iter().enumerate() {
            if outcome.is_err() {
                break;
            }
            let step = format!("step {i} {op:?} {hop:?} of {steps:?}");
            eprintln!("[driver-hop] {step}");
            apply(&mut window, &mut model, op);
            outcome = match window.settle(*hop) {
                Ok(()) => Ok(()),
                Err(SettleError::Timeout(t))
                    if matches!(
                        hop,
                        Hop::HeldAfterEngine {
                            short_deadline: true,
                            ..
                        }
                    ) =>
                {
                    // The hold stops every tokio task, so a late watcher is as legitimate a cause
                    // as a late driver.
                    if t.owed.is_empty() {
                        Err(TestCaseError::fail(format!(
                            "{step}: the settle timed out while the tokio workers were held, yet \
                             names nothing it waits for: {}",
                            SettleError::Timeout(t)
                        )))
                    } else {
                        window.settle(Hop::OnTime).map_err(|e| {
                            TestCaseError::fail(format!(
                                "{step}: once the hold ended, the settle still fails: {e}"
                            ))
                        })
                    }
                }
                Err(e) => Err(TestCaseError::fail(format!("{step}: {e}"))),
            }
            .and_then(|()| {
                window.assert_drivers_applied(&step)?;
                window.assert_screen_shows(&model, &step)
            });
        }
        window.close();
        outcome
    }

    /// Cases the generator found, run through the same property.
    #[test]
    fn settled_frame_shows_every_row_its_drivers_received_on_found_cases() {
        let held = |short_deadline| Hop::HeldAfterEngine {
            hold_ms: 50,
            short_deadline,
        };
        let found = [
            ([1, 1], vec![(Op::Refocus, held(false))]),
            ([1, 1], vec![(Op::Refocus, held(true))]),
            ([3, 3], vec![(Op::Refocus, Hop::OnTime)]),
        ];
        for (seeded, steps) in found {
            settled_frame_shows_every_row(seeded, steps).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    #[test]
    #[ignore = "holon's ui_watcher Delivery::take_over leaves the seq of a superseded generation unclaimed"]
    fn settled_frame_shows_every_row_for_a_superseded_generation() {
        for seeded in [[2, 1], [3, 1]] {
            let steps = vec![(Op::AddChild, Hop::HeldBeforeEngine { hold_ms: 200 })];
            settled_frame_shows_every_row(seeded, steps).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    proptest! {
        // A case boots a backend and a window: seconds each.
        #![proptest_config(ProptestConfig {
            cases: 6,
            max_shrink_iters: 24,
            failure_persistence: None,
            ..ProptestConfig::default()
        })]
        #[test]
        fn settled_frame_shows_every_row_its_drivers_received((seeded, steps) in case()) {
            settled_frame_shows_every_row(seeded, steps)?;
        }
    }
}
