//! `settle_window` over a scripted window: the engine's changes and the views'
//! snapshots arrive one per pump, with idle pumps in between, as a watcher
//! thread delivers them. Whatever the order, once every view has shown its
//! rows the settle returns a frame with no loading placeholder that reflects
//! every emitted change. A view's first snapshot carries no new engine seq, so
//! a frame can reflect the emitted seq while a view still loads. An admitted
//! operation emits its change only when it finishes, so a frame can reflect
//! the emitted seq while an operation's write is still to come.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result;
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
use crate::pbt_harness::settle::settle_window;

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
}

struct ScriptedWindow {
    engine: Arc<SeqEngine>,
    watermark: Arc<UiWatermark>,
    bounds: BoundsRegistry,
    views: Vec<UiSubscription>,
    shown: Vec<Cell<bool>>,
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
        for (k, shown) in self.shown.iter().enumerate() {
            let widget = match shown.get() {
                true => "live_block",
                false => "loading",
            };
            self.bounds.record(format!("view-{k}"), element(widget));
        }
    }
}

impl ScriptedWindow {
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
        }
        self.draw();
    }
}

fn element(widget: &str) -> ElementInfo {
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
        displayed_text: None,
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

fn arrival(n: usize) -> impl Strategy<Value = Arrival> {
    prop_oneof![
        Just(Arrival::Idle),
        (0..n).prop_map(Arrival::Route),
        (0..n).prop_map(Arrival::Apply),
        Just(Arrival::Admit),
        (0..n).prop_map(Arrival::Finish),
    ]
}

/// Random arrivals, then the end of every admitted operation and every view's
/// final snapshot.
fn schedule() -> impl Strategy<Value = (usize, Vec<Arrival>)> {
    (1usize..4).prop_flat_map(|n| {
        proptest::collection::vec(arrival(n), 0..30).prop_map(move |mut arrivals| {
            let admitted = arrivals
                .iter()
                .filter(|a| matches!(a, Arrival::Admit))
                .count();
            arrivals.extend((0..admitted).map(|_| Arrival::Finish(0)));
            arrivals.extend((0..n).map(Arrival::Apply));
            (n, arrivals)
        })
    })
}

fn settles_on_every_shown_view(n: usize, arrivals: Vec<Arrival>) -> Result<(), TestCaseError> {
    let window = ScriptedWindow::new(n, arrivals.clone());
    let epoch = settle_window(
        &window,
        &window.bounds,
        &window.watermark,
        &mut || {},
        Duration::from_secs(5),
    )
    .map_err(|e| TestCaseError::fail(format!("{arrivals:?}: {e}")))?;
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

mod perpetual_notify_never_settles {
    use gpui::AppContext as _;
    use gpui::HeadlessAppContext;
    use gpui::IntoElement;
    use gpui::Render;
    use gpui::Window;

    use super::*;

    struct Spinner {
        bounds: BoundsRegistry,
        watermark: Arc<UiWatermark>,
        renders: Arc<AtomicUsize>,
    }

    impl Render for Spinner {
        fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
            self.renders.fetch_add(1, Ordering::SeqCst);
            self.bounds.begin_pass(self.watermark.current());
            self.bounds
                .record("spinner".to_string(), element("live_block"));
            cx.notify();
            gpui::div()
        }
    }

    #[test]
    fn settle_window_refuses_a_view_that_notifies_in_every_render() {
        let renders = Arc::new(AtomicUsize::new(0));
        {
            let renders = renders.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(40));
                eprintln!(
                    "perpetual-notify HANG: settle_window did not return within 40 s; renders so far {}",
                    renders.load(Ordering::SeqCst)
                );
                std::process::exit(42);
            });
        }
        let mut app = HeadlessAppContext::new(gpui_platform::current_text_system());
        let engine = Arc::new(SeqEngine::default());
        let watermark = UiWatermark::new(Some(engine));
        let bounds = BoundsRegistry::new();
        eprintln!("perpetual-notify opening window");
        let (b, w, r) = (bounds.clone(), watermark.clone(), renders.clone());
        app.open_window(
            gpui::size(gpui::px(400.0), gpui::px(300.0)),
            move |_, cx| {
                cx.new(|_| Spinner {
                    bounds: b,
                    watermark: w,
                    renders: r,
                })
            },
        )
        .expect("open_window");
        eprintln!(
            "perpetual-notify window open, renders {}",
            renders.load(Ordering::SeqCst)
        );
        let r = settle_window(
            &app,
            &bounds,
            &watermark,
            &mut || {},
            Duration::from_secs(3),
        );
        eprintln!(
            "perpetual-notify renders {}, result: {r:?}",
            renders.load(Ordering::SeqCst)
        );
        match r {
            Err(e) => eprintln!("perpetual-notify display: {e}"),
            Ok(ep) => panic!("perpetual-notify settled Ok under a perpetual cx.notify: {ep:?}"),
        }
    }
}
