//! Shared body of the sim (TestPlatform) random GPUI PBT — same proptest
//! state machine, signature pinning, and capture format as `random_pbt.rs`,
//! but drives the window through `SimReplayer` instead of `WindowHost`.

use std::cell::RefCell;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui::{AssetSource, PlatformTextSystem, TestApp};
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::fixtures::FixtureStep;
use holon_integration_tests::pbt::state_machine::{fresh_reference_state, ReferenceMachine};
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::ui_harness::{
    set_loro_peer_id_if_unset, set_memory_multiplier_if_unset, standard_pbt_config,
};
use holon_integration_tests::pbt::ReferenceState;
use holon_integration_tests::test_environment::TestEnvironment;
use proptest::strategy::{BoxedStrategy, Just, Strategy};
use proptest::test_runner::{TestCaseError, TestRunner};
use proptest_state_machine::ReferenceStateMachine;

use super::sim_windowed_replay::SimReplayer;

static WIRING: OnceLock<holon_pbt_core::Wiring> = OnceLock::new();

struct WindowedRefMachine;

impl ReferenceStateMachine for WindowedRefMachine {
    type State = ReferenceState;
    type Transition = E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        let wiring = WIRING
            .get()
            .expect("random_pbt_sim::run must set WIRING before building the strategy")
            .clone();
        Just(fresh_reference_state(wiring)).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        ReferenceMachine::transitions(state)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        ReferenceMachine::preconditions(state, transition)
    }

    fn apply(state: Self::State, transition: &Self::Transition) -> Self::State {
        ReferenceMachine::apply(state, transition)
    }
}

fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    let platform = gpui_platform::current_platform(true);
    platform.text_system()
}

pub fn run(wiring: holon_pbt_core::Wiring, slice_name: &'static str) {
    WIRING
        .set(wiring.clone())
        .expect("random_pbt_sim::run called twice in one process");

    set_memory_multiplier_if_unset("15");
    set_loro_peer_id_if_unset("1");
    for (k, v) in [
        ("PBT_ATOMIC_EDITOR", "1"),
        ("PBT_MUTABLE_TEXT", "1"),
        ("PBT_REAL_EDITOR", "1"),
        ("PBT_PAUSE_SECONDS", "0"),
    ] {
        if std::env::var(k).is_err() {
            unsafe { std::env::set_var(k, v) };
        }
    }

    let num_steps: usize = std::env::var("PBT_NUM_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    let mut config = standard_pbt_config(slice_name);
    config.cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let total_cases = config.cases;

    let needle = std::env::var("HOLON_MINIMIZE_SIGNATURE")
        .unwrap_or_else(|_| "trouble begins at:".to_string());
    let capture_name =
        std::env::var("HOLON_PBT_CAPTURE_NAME").unwrap_or_else(|_| slice_name.to_string());

    // ---- TestPlatform setup ----
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

    // Backend for the window's own spawns.
    let window_rt = tokio::runtime::Runtime::new().expect("window-lifetime tokio runtime");
    let _window_rt_guard = window_rt.enter();

    // Open the window using the existing TestEnvironment pattern.
    let start_rt = Arc::new(tokio::runtime::Runtime::new().expect("start tokio runtime"));
    let mut env = start_rt.block_on(async { TestEnvironment::new(start_rt.clone()).unwrap() });
    start_rt.block_on(async { env.start_app(true).await.expect("start_app") });

    let session = env.session_arc();
    let engine = env.reactive_engine.clone().expect("reactive engine");
    let debug = env.debug_services().cloned().expect("debug services");
    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();

    let rebind_handle = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                start_rt.handle().clone(),
                nav,
                bounds.clone(),
                Some(debug.clone()),
                "Holon PBT Sim",
                cx,
            )
        })
        .expect("window opened");

    // Build the sim replayer — the SimReplayer owns TestApp;
    // SimUserDriver instances borrow it via raw pointer.
    let replayer = SimReplayer::new(
        app,
        rebind_handle,
        bounds.clone(),
        start_rt.handle().clone(),
    );

    // ---- Proptest loop (inline, no bg thread) ----
    let mut runner = TestRunner::new(config);
    let pinned: RefCell<Option<String>> = RefCell::new(None);

    let strategy = WindowedRefMachine::sequential_strategy(num_steps);
    let result = runner.run(&strategy, |(_initial_ref, transitions, seen_counter)| {
        holon_integration_tests::pbt::slice::reset_capture("gpui-sim");
        holon_integration_tests::pbt::slice::record_capture_wiring(
            WIRING.get().expect("WIRING set above"),
        );
        for t in &transitions {
            holon_integration_tests::pbt::slice::record_transition(t);
        }
        let steps: Vec<FixtureStep> = transitions
            .iter()
            .cloned()
            .map(FixtureStep::Action)
            .collect();

        let replay_wiring = WIRING.get().expect("WIRING set above").clone();
        match replayer.replay(replay_wiring, steps, seen_counter) {
            Ok(()) => Ok(()),
            Err(payload) => {
                let msg = super::panic_message(&payload);
                let first_failure = pinned.borrow().is_none();
                if first_failure {
                    eprintln!(
                        "[{slice_name}] pinned reproduction signature (needle={needle:?}); \
                         first failure:\n{msg}"
                    );
                    *pinned.borrow_mut() = Some(msg.clone());
                }
                if first_failure || msg.contains(&needle) {
                    holon_integration_tests::pbt::slice::write_captured_fixture(&capture_name);
                    let head: String = msg.chars().take(200).collect();
                    Err(TestCaseError::fail(head))
                } else {
                    eprintln!("[{slice_name}] shrink candidate panicked off-signature; ignoring");
                    Ok(())
                }
            }
        }
    });

    match result {
        Ok(()) => eprintln!("[{slice_name}] all {total_cases} case(s) passed"),
        Err(e) => panic!("[{slice_name}] PBT failed (shrunk): {e}"),
    }

    // Cleanup (SimReplayer owns TestApp; its Drop cleans up).
    drop(replayer);
}
