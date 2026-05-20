//! Shared body of the random windowed GPUI PBT — one entry point, two test
//! targets selecting the wiring explicitly:
//!
//! - `gpui_ui_pbt` — `Wiring::full()`: Loro enabled, editor edits persist
//!   per-keystroke.
//! - `gpui_ui_pbt_no_loro` — `Wiring::sql_only()`: Turso storage, Loro
//!   disabled, content persisted only by the on-blur `set_field`.
//!
//! Each variant is its own `[[test]]` binary so both run automatically (no
//! env-var opt-in). See `gpui_ui_pbt.rs` for the full harness docs
//! (generation + shrinking, signature pinning).

use std::cell::RefCell;
use std::sync::OnceLock;

use holon_integration_tests::pbt::fixtures::FixtureStep;
use holon_integration_tests::pbt::state_machine::{fresh_reference_state, ReferenceMachine};
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::ui_harness::{
    set_loro_peer_id_if_unset, set_memory_multiplier_if_unset, standard_pbt_config,
};
use holon_integration_tests::pbt::ReferenceState;
use proptest::strategy::{BoxedStrategy, Just, Strategy};
use proptest::test_runner::{TestCaseError, TestRunner};
use proptest_state_machine::ReferenceStateMachine;

use super::windowed_replay::windowed_replay_service;

/// Wiring for this process's run. `ReferenceStateMachine::init_state` takes no
/// arguments, so the slice's wiring is published here by [`run`] before any
/// strategy is built. One mode per process — each wiring variant is its own
/// test binary.
static WIRING: OnceLock<holon_pbt_core::Wiring> = OnceLock::new();

/// Reference machine for the windowed proptest run: identical alphabet,
/// weights, preconditions and apply to [`ReferenceMachine`], but `init_state`
/// honors the slice's [`WIRING`] (`ReferenceMachine::init_state` hardcodes
/// `Wiring::full()`). `init_state` stays a per-variant `Just(..)`, so
/// init-state shrinking is a harmless no-op (no wiring flip mid-shrink).
struct WindowedRefMachine;

impl ReferenceStateMachine for WindowedRefMachine {
    type State = ReferenceState;
    type Transition = E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        let wiring = WIRING
            .get()
            .expect("random_pbt::run must set WIRING before building the strategy")
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

pub fn run(wiring: holon_pbt_core::Wiring, slice_name: &'static str) {
    WIRING
        .set(wiring.clone())
        .expect("random_pbt::run called twice in one process");

    // The windowed real-editor harness: a real GPUI `GpuiUserDriver` drives a
    // live `InputState`. Replicate the env the old `run_in_gpui_window` +
    // `run_pbt_with_driver_sync_callback` path set (memory budget for GPUI's
    // ~40MB/transition, atomic editor / mutable text primitives, a Loro peer,
    // and the real-editor flag that ungates SqlOnly editor typing + commits
    // content on blur). `standard_pbt_config` enables the atomic editor + the
    // rejection-histogram panic hook.
    set_memory_multiplier_if_unset("15");
    set_loro_peer_id_if_unset("1");
    for (k, v) in [
        ("PBT_ATOMIC_EDITOR", "1"),
        ("PBT_MUTABLE_TEXT", "1"),
        ("PBT_REAL_EDITOR", "1"),
        // No per-step pause: shrinking replays many candidates.
        ("PBT_PAUSE_SECONDS", "0"),
    ] {
        if std::env::var(k).is_err() {
            unsafe { std::env::set_var(k, v) };
        }
    }

    eprintln!("[{slice_name}] wiring: {wiring:?}");

    let num_steps: usize = std::env::var("PBT_NUM_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    // Windowed replays are expensive, so default to a single generated case
    // (shrinking only fires on failure). `PROPTEST_CASES` overrides; persistence
    // + shrink budget come from `standard_pbt_config`. The slice name keeps the
    // regressions file separate per variant and from the headless slices.
    let mut config = standard_pbt_config(slice_name);
    config.cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let total_cases = config.cases;

    // The signature a *shrink candidate*'s panic must contain to count as a
    // reproduction (else proptest would drift to a different failure). Default
    // the `format_layer_report` marker; override with `HOLON_MINIMIZE_SIGNATURE`.
    // The FIRST failure of a case always fails the run regardless of signature
    // — an off-signature failure is still a failure (it just won't shrink).
    let needle = std::env::var("HOLON_MINIMIZE_SIGNATURE")
        .unwrap_or_else(|_| "trouble begins at:".to_string());
    let capture_name =
        std::env::var("HOLON_PBT_CAPTURE_NAME").unwrap_or_else(|_| slice_name.to_string());

    // The replayer is consumed only by the bg closure (moved in), so it needs no
    // Arc — and `WindowedReplayer` holds an `mpsc::Receiver` (`!Sync`), so an
    // `Arc<WindowedReplayer>` wouldn't be `Send` anyway.
    let (host, replayer) = windowed_replay_service();

    let bg = move || {
        let mut runner = TestRunner::new(config);
        // Pinned on the first failure of this run; never cleared. Once a case
        // fails, proptest only shrinks it (no new cases), so every later closure
        // call is a shrink candidate of that same failure.
        let pinned: RefCell<Option<String>> = RefCell::new(None);

        let strategy = WindowedRefMachine::sequential_strategy(num_steps);
        let result = runner.run(&strategy, |(_initial_ref, transitions, seen_counter)| {
            // Arm the capture for THIS candidate so a reproducing panic writes the
            // candidate's (shrunk) sequence to
            // `tests/.captures/<name>.captured.json` — the interchange format the
            // windowed minimizer + lattice bisection replay. Re-armed per
            // candidate so the final (minimal) reproduction overwrites earlier,
            // larger ones.
            holon_integration_tests::pbt::slice::reset_capture("gpui");
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
                        // The first failure ALWAYS fails the run — swallowing an
                        // off-signature failure as Ok(()) would make proptest
                        // report "all cases passed" for a run that panicked.
                        // Off-needle failures simply won't shrink (every shrink
                        // candidate backs off), so the original sequence is
                        // reported and captured as-is.
                        holon_integration_tests::pbt::slice::write_captured_fixture(&capture_name);
                        // Keep the proptest failure message short but identifiable.
                        let head: String = msg.chars().take(200).collect();
                        Err(TestCaseError::fail(head))
                    } else {
                        // Off-signature *shrink candidate* — back off this branch.
                        eprintln!(
                            "[{slice_name}] shrink candidate panicked off-signature; ignoring"
                        );
                        Ok(())
                    }
                }
            }
        });

        match result {
            Ok(()) => eprintln!("[{slice_name}] all {total_cases} case(s) passed"),
            // The reproducing candidate already wrote its capture; panic so the
            // window host re-raises and the test process exits non-zero.
            Err(e) => panic!("[{slice_name}] PBT failed (shrunk): {e}"),
        }
    };

    host.run_window("Holon PBT", bg);
}
