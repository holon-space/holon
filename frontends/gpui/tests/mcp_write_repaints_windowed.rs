//! A block created over the MCP `execute_operation` tool must become
//! clickable without an unrelated navigation.
//!
//! Bugfunnel `2026-08-31-mcp-created-block-never-paints-until-navigation`:
//! the write reached SQL and the view model, but `click_entity` answered
//! "element bounds never committed" until a `navigation.focus` away and back
//! repainted the window. Gesture-originated writes never showed it, because a
//! gesture is itself a platform input and the window paints on the way out.
//!
//! The gap is DRIVER PARITY, and it lives in `GpuiUserDriver` — the driver the
//! MCP tools inject into (the composed keystone drives `SimUserDriver`, which
//! pumps the whole app on every read and so cannot reach this). Bounds are
//! render-derived state: `await_editor_window_focus` already DRAWS the frames
//! it reads (`InteractionEvent::ForceFrame`), but the bounds resolution ahead
//! of it polled passively. A write that arrived off the input path schedules
//! no frame of its own, so that poll burned its whole budget waiting for a
//! frame nobody was going to draw.
//!
//! ## What the harness models, and why it has to
//!
//! gpui's TestPlatform draws every dirty window inside `flush_effects`, so a
//! test that pumps the app at all repaints ambiently and the bug is
//! structurally invisible — which is why the entry is filed as an ENVIRONMENT
//! gap. [`DrivenFrameGeometry`] restores the missing environment fact: the
//! driver reads a SNAPSHOT of the bounds registry that is republished only
//! after the driver sends an event whose pump arm actually calls
//! `window.draw()` ([`draws_a_frame`]). Events that merely `refresh()` change
//! nothing the driver can see, which is what an occluded production window
//! does.
//!
//! Keying the gate on the EVENT rather than on an observed draw is forced:
//! the harness must pump the gpui app for the driver's channel round-trip at
//! all, and pumping draws every dirty window, so no frame counter could ever
//! be attributed to the driver's own command.
//!
//! ## The three rungs
//!
//! * [`an_mcp_created_block_is_clickable_without_a_navigation`] — the bug, with
//!   a keyboard write as the inverse control.
//! * [`only_a_drawing_event_reveals_an_mcp_created_row`] — the negative twin:
//!   the same wait shape driven by a non-drawing event must NOT reveal the row,
//!   so a regression that swaps `ForceFrame` for any other dispatch is caught.
//! * [`a_never_painted_entity_still_fails_loud_at_the_bounds_deadline`] — the
//!   forced draws must not soften or extend the loud timeout arm.
//!
//! Run: `cargo nextest run -p holon-gpui --test mcp_write_repaints_windowed
//! --features holon-gpui/pbt -j1`

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::FrontendSession;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::RebindHandle;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::user_driver::GpuiUserDriver;
use holon_integration_tests::pbt::window_slice::seed::GRAFT_PAGE_ID;
use holon_integration_tests::pbt::window_slice::seed::PROMOTION_TARGET_CONTENT;
use holon_integration_tests::pbt::window_slice::seed::PROMOTION_TARGET_ID;
use holon_integration_tests::pbt::window_slice::seed::graft_promotion_target_row;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_mcp::server::InteractionCommand;
use holon_mcp::server::InteractionEvent;

/// Content of every block these rungs create over the MCP operation path.
/// Distinct from anything the default vault or the graft fixture ships, so a
/// painted element carrying it can only be one of ours.
const SENTINEL_CONTENT: &str = "mcp-origin row must paint";

/// The character the inverse control types into the seeded row. Absent from
/// [`PROMOTION_TARGET_CONTENT`], so finding it in painted text proves the
/// keystroke reached the screen rather than matching what was already there.
const CONTROL_CHAR: &str = "Z";

/// The driver's own budget for a bounds resolution
/// (`user_driver.rs::CLICK_BOUNDS_TIMEOUT`). Restated here because the loud
/// arm's rung asserts against it and the constant is crate-private.
const CLICK_BOUNDS_TIMEOUT: Duration = Duration::from_secs(5);

fn real_text_system() -> Arc<dyn gpui::PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

/// Does this event reach a pump arm that DRAWS?
///
/// `ForceFrame` is the only one: several arms call `window.refresh()`, which
/// merely marks the window dirty, but `window.draw()` — the call that produces
/// the frame render-derived state is read from — is reached from
/// `InteractionEvent::ForceFrame` alone (`frontends/gpui/src/lib.rs`, the
/// `ForceFrame` arm of the interaction pump). If a new drawing arm is ever
/// added there, it belongs in this match too, or these rungs will under-report
/// what a driver can see.
fn draws_a_frame(event: &InteractionEvent) -> bool {
    matches!(event, InteractionEvent::ForceFrame)
}

/// One pump cycle: real tokio time for the backend watchers, drain gpui tasks
/// (test builds draw dirty windows inside `flush_effects`), fire fake timers,
/// drain again, promote staged bounds.
fn pump(app: &mut HeadlessAppContext, bounds: &BoundsRegistry) {
    std::thread::sleep(Duration::from_millis(10));
    app.run_until_parked();
    app.advance_clock(Duration::from_millis(500));
    app.run_until_parked();
    bounds.flush();
}

/// The bounds a driver may read: those of the last frame the driver itself
/// caused to be DRAWN. The ambient pumping this harness needs still repaints
/// the window, but nothing of that reaches a reader here until an event
/// [`draws_a_frame`] accepts has gone through — which is what an occluded
/// production window gives a driver.
#[derive(Clone)]
struct DrivenFrameGeometry {
    live: BoundsRegistry,
    snapshot: Arc<Mutex<HashMap<String, ElementInfo>>>,
}

impl DrivenFrameGeometry {
    fn new(live: BoundsRegistry) -> Self {
        Self {
            live,
            snapshot: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Publish the current frame to readers. Called once at boot (the window
    /// painted when it opened) and after every drawing event.
    fn capture(&self) {
        self.live.flush();
        *self.snapshot.lock().unwrap() = self.live.all_elements().into_iter().collect();
    }
}

impl GeometryProvider for DrivenFrameGeometry {
    fn element_info(&self, id: &str) -> Option<ElementInfo> {
        self.snapshot.lock().unwrap().get(id).cloned()
    }

    fn all_elements(&self) -> Vec<(String, ElementInfo)> {
        self.snapshot
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn clone_box(&self) -> Box<dyn GeometryProvider> {
        Box::new(self.clone())
    }
}

/// Everything `entity` paints, read from a registry, as `(element id, text)`.
fn painted_texts(elements: &[(String, ElementInfo)], entity: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = elements
        .iter()
        .filter(|(_, info)| info.entity_id.as_deref() == Some(entity))
        .filter(|(_, info)| matches!(info.widget_type.as_ref(), "rendered_text" | "editable_text"))
        .filter_map(|(id, info)| {
            info.displayed_text
                .as_ref()
                .map(|t| (id.clone(), t.to_string()))
        })
        .collect();
    out.sort();
    out
}

/// A booted window with a real engine, a real interaction pump, and the
/// driver's view of it gated through [`DrivenFrameGeometry`].
struct Fixture {
    app: HeadlessAppContext,
    bounds: BoundsRegistry,
    geometry: Arc<DrivenFrameGeometry>,
    /// The driver's end of the proxy. Cloned so a rung can push an event of
    /// its own through the exact path the driver's events take.
    proxy_tx: futures::channel::mpsc::Sender<InteractionCommand>,
    proxy_rx: futures::channel::mpsc::Receiver<InteractionCommand>,
    window_tx: futures::channel::mpsc::Sender<InteractionCommand>,
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    rebind: RebindHandle,
    _env: TestEnvironment,
    _runtime: Arc<tokio::runtime::Runtime>,
}

impl Fixture {
    /// Forward everything the driver has queued, then advance the app. A
    /// forwarded event republishes the driver's view only when it draws.
    fn pump_and_forward(&mut self) {
        while let Ok(cmd) = self.proxy_rx.try_recv() {
            let draws = draws_a_frame(&cmd.event);
            self.window_tx
                .try_send(cmd)
                .unwrap_or_else(|e| panic!("the window interaction pump refused a command: {e}"));
            pump(&mut self.app, &self.bounds);
            if draws {
                self.geometry.capture();
            }
        }
        pump(&mut self.app, &self.bounds);
    }

    /// Push one event through the same proxy the driver uses, so the draw gate
    /// treats it exactly as it treats the driver's own, and wait for the pump
    /// to answer.
    fn dispatch_raw(&mut self, event: InteractionEvent, label: &str) {
        let (response_tx, mut response_rx) = tokio::sync::oneshot::channel();
        self.proxy_tx
            .try_send(InteractionCommand { event, response_tx })
            .unwrap_or_else(|e| panic!("{label}: proxy channel: {e}"));
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            self.pump_and_forward();
            if response_rx.try_recv().is_ok() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{label}: the window pump never answered"
            );
        }
    }

    /// Create a block the way the MCP `execute_operation` tool does: straight
    /// at the session, with no platform input anywhere.
    fn create_over_mcp(&self, id: &str) -> EntityUri {
        let uri = EntityUri::block(id);
        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert("id".into(), Value::String(uri.to_string()));
        params.insert(
            "parent_id".into(),
            Value::String(EntityUri::block(GRAFT_PAGE_ID).to_string()),
        );
        params.insert("content".into(), Value::String(SENTINEL_CONTENT.into()));
        params.insert("content_type".into(), holon_api::ContentType::Text.into());
        futures::executor::block_on(self.session.execute_operation(
            &EntityName::new("block"),
            "create",
            params,
        ))
        .expect("the MCP-origin create must land");
        uri
    }

    /// Pump until `entity` has a text element in the LIVE registry, so a later
    /// bounds failure names visibility rather than a write that never rendered.
    fn settle_until_painted(&mut self, entity: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            pump(&mut self.app, &self.bounds);
            if !painted_texts(&self.bounds.all_elements(), entity).is_empty() {
                return true;
            }
        }
        false
    }

    /// Is `entity` resolvable in the view the DRIVER reads (as opposed to the
    /// window's live registry)?
    fn driver_sees(&self, entity: &str) -> bool {
        self.geometry.find_by_entity_id_visible(entity).is_some()
    }

    fn shutdown(mut self) {
        drop(self.rebind);
        self.app.update(|cx| cx.shutdown());
        self.app.run_until_parked();
        std::mem::forget(self.app);
    }
}

/// Run a driver future on the gpui thread, forwarding its interaction events
/// to the real window pump.
///
/// The driver is async over a tokio channel while gpui owns this thread, so
/// the future is polled inline and the app is pumped between polls.
fn drive<T>(fx: &mut Fixture, fut: impl Future<Output = T>, label: &str) -> T {
    let waker = futures::task::noop_waker();
    let mut task_cx = std::task::Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if let std::task::Poll::Ready(value) = fut.as_mut().poll(&mut task_cx) {
            return value;
        }
        fx.pump_and_forward();
        assert!(
            Instant::now() < deadline,
            "{label}: the driver future never completed"
        );
    }
}

/// Boot a window over a seeded vault, hand the body a fixture and the real
/// `GpuiUserDriver` reading through the gated geometry, then shut down.
fn with_fixture(title: &str, body: impl FnOnce(&mut Fixture, &GpuiUserDriver)) {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    // Writes driven from the gpui thread reach Loro's `emit_change`, which
    // `tokio::spawn`s and therefore needs an entered reactor. The guard is held
    // for the whole test, so every future here is driven by a non-tokio
    // executor — `Runtime::block_on` panics inside an entered guard.
    let _reactor = runtime.enter();
    let env = futures::executor::block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    futures::executor::block_on(async { env.start_app(true).await.expect("start_app") });
    futures::executor::block_on(graft_promotion_target_row(&env))
        .expect("graft a painted row under the Main focus root");

    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let debug_services = env.debug_services().cloned().expect("debug services");

    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();
    let rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                Some(debug_services.clone()),
                None,
                title,
                cx,
            )
        })
        .expect("window opened");

    let window_tx = debug_services
        .interaction_tx
        .get()
        .expect("interaction_tx set by the window interaction pump")
        .clone();
    let (proxy_tx, proxy_rx) = futures::channel::mpsc::channel::<InteractionCommand>(64);
    let geometry = Arc::new(DrivenFrameGeometry::new(bounds.clone()));
    let driver = GpuiUserDriver::new(proxy_tx.clone(), geometry.clone(), engine.clone());

    let mut fx = Fixture {
        app,
        bounds,
        geometry,
        proxy_tx,
        proxy_rx,
        window_tx,
        session,
        engine,
        rebind,
        _env: env,
        _runtime: runtime.clone(),
    };

    let seeded_element = format!("block:{PROMOTION_TARGET_ID}");
    assert!(
        fx.settle_until_painted(&seeded_element, Duration::from_secs(180)),
        "boot precondition: the seeded row must paint before anything is driven"
    );
    // The window painted when it opened; that frame is what the driver starts
    // from, exactly as an agent attaching to a running app does.
    fx.geometry.capture();

    body(&mut fx, &driver);

    drop(driver);
    fx.shutdown();
}

#[test]
fn an_mcp_created_block_is_clickable_without_a_navigation() {
    with_fixture("Holon-TestPlatform-McpPaint", |fx, driver| {
        // ── the inverse control: a keyboard write, no navigation ──────────
        let seeded = EntityUri::from_raw(PROMOTION_TARGET_ID);
        let seeded_element = format!("block:{PROMOTION_TARGET_ID}");
        drive(
            fx,
            driver.click_entity(&seeded, "main"),
            "control: click the seeded row",
        )
        .expect("the seeded row is in the boot frame, so its bounds resolve at once");
        drive(
            fx,
            driver.send_raw_keystroke(CONTROL_CHAR, &[]),
            "control: type into the seeded row",
        )
        .expect("typing into the focused row");

        let control_deadline = Instant::now() + Duration::from_secs(60);
        let mut control_painted = Vec::new();
        while Instant::now() < control_deadline {
            pump(&mut fx.app, &fx.bounds);
            control_painted = painted_texts(&fx.bounds.all_elements(), &seeded_element);
            if control_painted
                .iter()
                .any(|(_, t)| t.contains(CONTROL_CHAR))
            {
                break;
            }
        }
        assert!(
            control_painted
                .iter()
                .any(|(_, t)| t.contains(CONTROL_CHAR)),
            "CONTROL: a keyboard write must reach the screen with no navigation. The row still \
             paints {control_painted:?} (seeded content {PROMOTION_TARGET_CONTENT:?})"
        );

        // ── the bug: a write that arrives off the input path ──────────────
        let sentinel = fx.create_over_mcp("mcp-paint-sentinel-row");
        let sentinel_element = sentinel.as_str().to_string();
        assert!(
            fx.settle_until_painted(&sentinel_element, Duration::from_secs(120)),
            "the MCP-origin row never rendered at all — that is a write/projection failure, not \
             the repaint-notification bug this rung is about"
        );

        // The driver's last drawn frame predates the create, and nothing about
        // the create asked for another one. Resolving the row's bounds is
        // therefore the driver's own job.
        let click = drive(
            fx,
            driver.click_entity(&sentinel, "main"),
            "click the MCP-created row",
        );
        assert!(
            click.is_ok(),
            "a block created over the MCP operation path must be clickable without an unrelated \
             navigation. The row IS painted in the window ({:?}), but the driver never drew a \
             frame while waiting for its bounds, so it saw only the boot frame: {}",
            painted_texts(&fx.bounds.all_elements(), &sentinel_element),
            click.unwrap_err()
        );
    });
}

/// The negative twin of the rung above: the wait has to DRAW, not merely
/// dispatch.
///
/// Reverting the production fix is not the only way to reintroduce the bug —
/// swapping `ForceFrame` for any other event reintroduces it just as
/// completely, because no other pump arm draws. This rung pushes a
/// non-drawing event (`ScrollList` at an entity that does not exist — its arm
/// calls `window.refresh()` and answers `handled=false`) through the driver's
/// own channel and pins that it reveals nothing, then pins that one
/// `ForceFrame` does.
#[test]
fn only_a_drawing_event_reveals_an_mcp_created_row() {
    with_fixture("Holon-TestPlatform-McpPaint-Negative", |fx, _driver| {
        let sentinel = fx.create_over_mcp("mcp-paint-negative-row");
        let sentinel_element = sentinel.as_str().to_string();
        assert!(
            fx.settle_until_painted(&sentinel_element, Duration::from_secs(120)),
            "the MCP-origin row never rendered at all — the probe below would be vacuous"
        );
        assert!(
            !fx.driver_sees(&sentinel_element),
            "precondition: the row must be invisible to the driver before any event is sent"
        );

        for attempt in 0..8 {
            fx.dispatch_raw(
                InteractionEvent::ScrollList {
                    entity_id: "block:mcp-paint-no-such-entity".into(),
                    dx: 0.0,
                    dy: 0.0,
                },
                "non-drawing probe",
            );
            assert!(
                !fx.driver_sees(&sentinel_element),
                "a NON-DRAWING event revealed the row on attempt {attempt}. The wait would then \
                 pass on any channel dispatch, which is not the contract the fix implements — \
                 the row IS painted in the window ({:?}), but only a real draw may publish it.",
                painted_texts(&fx.bounds.all_elements(), &sentinel_element)
            );
        }

        fx.dispatch_raw(InteractionEvent::ForceFrame, "drawing probe");
        assert!(
            fx.driver_sees(&sentinel_element),
            "one ForceFrame must publish the painted row — without this the negative assertion \
             above would hold vacuously. Window paints {:?}",
            painted_texts(&fx.bounds.all_elements(), &sentinel_element)
        );
    });
}

/// The forced draws must not soften the loud arm.
///
/// A row that genuinely never renders still has to fail at
/// `CLICK_BOUNDS_TIMEOUT` with the rich diagnostic, and the engine's stale
/// focus still has to be cleared so a following keystroke cannot silently
/// mutate the previously-focused block.
#[test]
fn a_never_painted_entity_still_fails_loud_at_the_bounds_deadline() {
    with_fixture("Holon-TestPlatform-McpPaint-LoudArm", |fx, driver| {
        // Seat focus first, so the stale-focus clear has something to clear.
        let seeded = EntityUri::from_raw(PROMOTION_TARGET_ID);
        drive(
            fx,
            driver.click_entity(&seeded, "main"),
            "seat focus on the seeded row",
        )
        .expect("the seeded row is in the boot frame");
        assert!(
            fx.engine
                .ui_state()
                .focused_block_mutable()
                .get_cloned()
                .is_some(),
            "precondition: the click must have seated engine focus"
        );

        let ghost = EntityUri::block("mcp-paint-ghost-never-painted");
        let started = Instant::now();
        let outcome = drive(
            fx,
            driver.click_entity(&ghost, "main"),
            "click an entity that never rendered",
        );
        let elapsed = started.elapsed();

        let err = match outcome {
            Ok(()) => panic!(
                "clicking an entity that never rendered must FAIL — a driven frame cannot \
                 conjure bounds for a row the window does not have"
            ),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            err.contains("element bounds never committed") && err.contains("no bounds recorded"),
            "the loud arm must keep its rich diagnostic; got {err:?}"
        );
        assert!(
            elapsed >= CLICK_BOUNDS_TIMEOUT,
            "the wait must still spend its full {CLICK_BOUNDS_TIMEOUT:?} budget before giving up \
             (took {elapsed:?}) — a forced draw must not turn a slow row into an early failure"
        );
        assert!(
            elapsed < CLICK_BOUNDS_TIMEOUT * 2,
            "the per-iteration forced draws must not EXTEND the deadline; the wait took {elapsed:?}"
        );
        assert!(
            fx.engine
                .ui_state()
                .focused_block_mutable()
                .get_cloned()
                .is_none(),
            "the failed click must clear stale focus, so a following keystroke cannot silently \
             mutate the previously-focused block"
        );
    });
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
