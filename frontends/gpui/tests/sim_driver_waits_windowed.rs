//! The sim driver's waits, called through `SimUserDriver` on a real window.
//! While nothing changes, a wait does not try again. A keystroke interceptor
//! counts the keystroke wait's tries. A try of the focus waits does not
//! settle, and a settle advances the app's timer clock by one step, so the
//! clock counts settles spent between tries. The scroll wait ends with the
//! interaction pump's reply, however late it comes.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use futures::StreamExt;
use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_api::EntityUri;
use holon_api::Key;
use holon_api::KeyChord;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::window_slice::seed::CHORD_TARGET_ID;
use holon_integration_tests::pbt::window_slice::seed::graft_chord_target_row;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_mcp::server::InteractionCommand;
use holon_mcp::server::InteractionResponse;
use pbt_harness::sim_windowed_replay::SimUserDriver;
use proptest::prelude::*;

use crate::pbt_harness;
use crate::pbt_harness::settle::HANG_DEADLINE;
use crate::pbt_harness::settle::settle_window;

/// Settles a wait at rest may spend beyond its gestures: one try before the
/// wait and one after it, plus a spurious wake or two. A wait that polls by
/// the clock spends one per 10 ms.
const SLACK_SETTLES: u64 = 4;

/// The driver's re-clicks when a click does not move engine focus.
const FOCUS_CLICKS: u64 = 3;

/// A key the focused editor does not consume.
const UNCONSUMED_KEY: Key = Key::F(13);

/// The latest drawn reply: later than the 200 tries of 5 ms a scroll wait that
/// polls by the clock gives up after.
const LATEST_REPLY_MS: u64 = 2500;

#[derive(Clone, Copy, Debug)]
enum SimWait {
    /// A key no handler consumes, pressed into the focused chord target row.
    Keystroke,
    /// Clicks on the right sidebar's empty area, so engine focus never lands.
    FocusLanding,
    /// Engine focus on a block the window does not paint, so no editor ever
    /// takes window focus.
    WindowFocus,
    /// The interaction pump replies `reply_ms` after the request.
    ScrollReply { reply_ms: u64 },
}

impl SimWait {
    /// The settles of the case's gestures, which are not tries.
    fn gesture_settles(self) -> u64 {
        match self {
            SimWait::Keystroke | SimWait::WindowFocus => 0,
            SimWait::FocusLanding => FOCUS_CLICKS,
            SimWait::ScrollReply { .. } => 1,
        }
    }
}

fn sim_wait() -> impl Strategy<Value = SimWait> {
    prop_oneof![
        Just(SimWait::Keystroke),
        Just(SimWait::FocusLanding),
        Just(SimWait::WindowFocus),
        (0..=LATEST_REPLY_MS).prop_map(|reply_ms| SimWait::ScrollReply { reply_ms }),
    ]
}

struct Window {
    app: HeadlessAppContext,
    runtime: Arc<tokio::runtime::Runtime>,
    env: TestEnvironment,
    bounds: BoundsRegistry,
}

impl Window {
    fn settle(&self) {
        let engine = self
            .env
            .reactive_engine
            .get()
            .expect("reactive engine after start_app");
        settle_window(
            &self.app,
            &self.bounds,
            engine.ui_watermark(),
            &mut || self.runtime.block_on(self.env.settle()),
            Duration::from_secs(30),
        )
        .unwrap_or_else(|e| panic!("window settle: {e}"));
    }

    fn timer_clock(&self) -> Instant {
        self.app.background_executor().now()
    }
}

struct Fixture<'w> {
    window: &'w Window,
    driver: SimUserDriver,
    engine: Arc<holon_frontend::reactive::ReactiveEngine>,
    target: EntityUri,
    ghost: EntityUri,
    inert: EntityUri,
    reply_after: std::sync::mpsc::Sender<Duration>,
    step: Duration,
    keys: Rc<Cell<u64>>,
}

impl Fixture<'_> {
    fn press(&self, at: &EntityUri) -> Result<bool, anyhow::Error> {
        let root_id = holon_api::root_layout_block_uri();
        let root_tree = self.engine.snapshot_reactive(&root_id);
        self.window.runtime.block_on(self.driver.send_key_chord(
            &root_id,
            &root_tree,
            at,
            &KeyChord::new(&[UNCONSUMED_KEY]),
            Default::default(),
        ))
    }

    fn focus(&self, block: &EntityUri) {
        self.engine.set_focus_with_caret(block.clone(), 0);
        self.window.settle();
    }

    fn run(&self, wait: SimWait) -> Result<(), TestCaseError> {
        let press = match wait {
            SimWait::Keystroke => {
                self.focus(&self.target);
                Some((&self.target, "never consumed"))
            }
            SimWait::FocusLanding => {
                self.focus(&self.target);
                Some((&self.inert, "never moved focused_block"))
            }
            SimWait::WindowFocus => {
                self.focus(&self.ghost);
                Some((&self.ghost, "never took window focus"))
            }
            SimWait::ScrollReply { reply_ms } => {
                self.reply_after
                    .send(Duration::from_millis(reply_ms))
                    .expect("the replier thread is alive");
                None
            }
        };
        let before = self.window.timer_clock();
        let keys_before = self.keys.get();
        match press {
            Some((at, refusal)) => match self.press(at) {
                Ok(_) => prop_assert!(false, "{wait:?}: the press went through, so nothing waited"),
                Err(e) => prop_assert!(
                    format!("{e:#}").contains(refusal),
                    "{wait:?}: the press failed for another reason than the wait: {e:#}"
                ),
            },
            None => {
                let scrolled = self
                    .window
                    .runtime
                    .block_on(self.driver.scroll_to_entity(&self.target));
                if let Err(e) = scrolled {
                    prop_assert!(
                        false,
                        "{wait:?}: the scroll wait ended before the reply: {e:#}"
                    );
                }
            }
        }
        let settles = (self.window.timer_clock() - before).as_nanos() / self.step.as_nanos();
        let allowed = u128::from(wait.gesture_settles() + SLACK_SETTLES);
        prop_assert!(
            settles <= allowed,
            "{wait:?} settled {settles} times with nothing changing (at most {allowed}): \
             it tries again without waiting for a change"
        );
        let keys = self.keys.get() - keys_before;
        prop_assert!(
            keys <= 1 + SLACK_SETTLES,
            "{wait:?} dispatched {keys} keystrokes with nothing changing (at most {}): \
             it tries again without waiting for a change",
            1 + SLACK_SETTLES
        );
        Ok(())
    }
}

/// Every fixed case runs to its verdict first, so one red names each failing
/// wait; then the drawn cases run.
#[test]
fn sim_driver_waits_wait_for_their_change() {
    let text_system = gpui_platform::current_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime.block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    runtime.block_on(async { env.start_app(true).await.expect("start_app") });
    runtime
        .block_on(graft_chord_target_row(&env))
        .expect("graft the chord target row");
    let mut window = Window {
        app,
        runtime: runtime.clone(),
        env,
        bounds: BoundsRegistry::new(),
    };

    let session = window.env.session_arc();
    let engine = window
        .env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let debug_services = window
        .env
        .debug_services()
        .cloned()
        .expect("debug services");
    let bounds = window.bounds.clone();
    let rebind = window
        .app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                NavigationState::new(),
                bounds,
                Some(debug_services),
                None,
                "Holon-TestPlatform-SimDriverWaits",
                cx,
            )
        })
        .expect("window opened");

    let target_element = format!("block:{CHORD_TARGET_ID}");
    let boot_deadline = Instant::now() + HANG_DEADLINE;
    while !window
        .bounds
        .all_elements()
        .iter()
        .any(|(_, info)| info.entity_id.as_deref() == Some(target_element.as_str()))
    {
        assert!(
            Instant::now() < boot_deadline,
            "boot: the chord target row never painted"
        );
        window.settle();
    }

    let inert = EntityUri::parse("block:default-right-sidebar").expect("a block uri");
    assert!(
        window
            .bounds
            .all_elements()
            .iter()
            .any(
                |(_, info)| info.entity_id.as_deref() == Some(inert.as_str())
                    && info.has_visible_area()
            ),
        "the window paints the right sidebar"
    );

    let (interaction_tx, mut interactions) =
        futures::channel::mpsc::channel::<InteractionCommand>(1);
    let (reply_after, delays) = std::sync::mpsc::channel::<Duration>();
    let handle = runtime.handle().clone();
    let replier = std::thread::spawn(move || {
        while let Some(command) = handle.block_on(interactions.next()) {
            let delay = delays.recv().expect("each request has its reply delay");
            std::thread::sleep(delay);
            command
                .response_tx
                .send(InteractionResponse {
                    handled: true,
                    detail: None,
                    screenshot: None,
                })
                .unwrap_or_else(|_| panic!("the driver stopped waiting for its scroll reply"));
        }
    });

    // SAFETY: `window.app` outlives the driver and stays on this (gpui) thread.
    let driver = SimUserDriver::new(
        &window.app,
        rebind.window(),
        window.bounds.clone(),
        engine.clone(),
        runtime.handle().clone(),
        interaction_tx,
    );
    let keys = Rc::new(Cell::new(0u64));
    let counted = keys.clone();
    let interceptor = window
        .app
        .update(|cx| cx.intercept_keystrokes(move |_, _, _| counted.set(counted.get() + 1)));
    let before = window.timer_clock();
    driver.hover_point(1.0, 1.0);
    let step = window.timer_clock() - before;
    assert!(
        step > Duration::ZERO,
        "a driver settle advances the timer clock"
    );

    let fixture = Fixture {
        window: &window,
        driver,
        engine,
        // ALLOW(entity_uri_from_raw): the seed grafts this bare id; schemed
        // here.
        target: EntityUri::from_raw(CHORD_TARGET_ID),
        // ALLOW(entity_uri_from_raw): a block id no seed creates.
        ghost: EntityUri::from_raw("sim-wait-ghost"),
        inert,
        reply_after,
        step,
        keys,
    };
    let failures: Vec<String> = [
        SimWait::Keystroke,
        SimWait::FocusLanding,
        SimWait::WindowFocus,
        SimWait::ScrollReply {
            reply_ms: LATEST_REPLY_MS,
        },
        SimWait::ScrollReply { reply_ms: 0 },
    ]
    .into_iter()
    .filter_map(|wait| fixture.run(wait).err().map(|e| e.to_string()))
    .collect();
    assert!(
        failures.is_empty(),
        "fixed cases failed:\n{}",
        failures.join("\n")
    );
    proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 4,
        failure_persistence: None,
        ..ProptestConfig::default()
    })
    .run(&sim_wait(), |wait| fixture.run(wait))
    .unwrap();

    drop(fixture);
    drop(interceptor);
    replier.join().expect("the replier thread panicked");
    drop(rebind);
    window.app.update(|cx| cx.shutdown());
    window.app.run_until_parked();
    std::mem::forget(window);
}
