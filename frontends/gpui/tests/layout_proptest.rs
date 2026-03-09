//! Property-based scenario test combining random widget-tree generation
//! with randomized user-interaction sequences.
//!
//! Each generated `Scenario` carries two things:
//!
//!   - a `Blueprint` — a random `ReactiveViewModel` tree plus a list of
//!     `BlockHandle`s collected during generation (one per mode-switchable
//!     `BlockRef` in the tree);
//!   - a `Vec<UiInteraction>` — a random sequence of UI actions picked
//!     from the scenario's blueprint handles.
//!
//! The oracle is **end-state equivalence**: a scenario is rendered twice.
//!
//!   1. *Reference*: every blueprint handle starts in the mode that
//!      matches its action sequence's *final* target. No actions are
//!      replayed — just an initial render. Dump captured.
//!   2. *Test*: blueprint handles start in their default (index 0) modes.
//!      Render, then replay the action sequence one step at a time,
//!      running gpui's executor to quiescence after each step. Final dump
//!      captured.
//!
//! Asserting `reference_dump == test_dump` catches any state-transition
//! regression: stale entity cache, missing `cx.notify`, shell not
//! swapping `current_tree`, wrong stream wiring, etc. — the whole class
//! of bugs that manifest as "the UI doesn't update when the user clicks".
//!
//! Layout invariants (`assert_layout_ok`) additionally run after the
//! initial render and after each intermediate action, so any regression
//! that only manifests at a specific point in the action sequence is
//! caught with step-level granularity.
//!
//! When the scenario has no switchable handles and an empty action
//! sequence, the test collapses to the old pure-layout proptest — no
//! coverage regression.
//!
//! Failure cases print via `Scenario`'s `Debug` impl, which materializes
//! the blueprint once and pretty-prints it alongside the action list.
//!
//! Driven via `TestRunner` inside a `#[gpui::test]` function (rather than
//! via the `proptest!` macro) so we can keep a `&mut TestAppContext` live
//! across test cases. `TestRunner::run` requires its closure to be `Fn`,
//! so `cx` is smuggled through a `RefCell` — fine since the whole
//! pipeline is single-threaded.

mod support;

use std::cell::RefCell;

use gpui::{px, size, TestAppContext};
use holon_layout_testing::generators::arb_scenario;
use holon_layout_testing::scenario::StepInput;
use holon_layout_testing::Scenario;
use proptest::test_runner::{Config, TestCaseError, TestRunner};

use support::GpuiScenarioSession;

fn fixture_window_size() -> gpui::Size<gpui::Pixels> {
    size(px(800.0), px(600.0))
}

/// Drive a scenario through `holon_layout_testing::run_scenario`, translating
/// the shared `StepInput` vocabulary into GPUI session operations.
///
/// The shared runner owns the reference + test rendering loop, layout
/// invariants, and end-state equivalence — this function only provides the
/// GPUI-specific step closure.
fn run_scenario_gpui(cx: &mut TestAppContext, scenario: &Scenario) -> Result<(), TestCaseError> {
    let cx_cell: RefCell<&mut TestAppContext> = RefCell::new(cx);
    let window_size = fixture_window_size();
    let mut session: Option<GpuiScenarioSession> = None;

    holon_layout_testing::run_scenario(scenario, |input| {
        let mut cx = cx_cell.borrow_mut();
        match input {
            StepInput::Mount { vm, blocks } => {
                // Drop any prior session, flush close work, open a new window.
                session = None;
                cx.run_until_parked();
                let s = GpuiScenarioSession::open(*cx, vm, blocks, window_size);
                let snap = s.snapshot();
                session = Some(s);
                snap
            }
            StepInput::Apply(action) => {
                let s = session
                    .as_ref()
                    .expect("StepInput::Apply before StepInput::Mount");
                s.apply_action(*cx, action);
                s.snapshot()
            }
        }
    })
}

// ── The property test ─────────────────────────────────────────────────

#[gpui::test]
fn layout_invariants_hold_for_random_scenarios(cx: &mut TestAppContext) {
    // Budget: 48 cases at 5 ms–30 ms each for pure layout, plus a
    // second window render and up to 5 action replays when the scenario
    // has switchable handles. Still a few seconds wall-clock in total.
    let config = Config {
        cases: 48,
        max_shrink_iters: 128,
        ..Config::default()
    };
    let mut runner = TestRunner::new(config);

    let cx_cell: RefCell<&mut TestAppContext> = RefCell::new(cx);

    let result = runner.run(&arb_scenario(), |scenario| {
        let mut cx = cx_cell.borrow_mut();
        run_scenario_gpui(*cx, &scenario)?;
        Ok(())
    });

    if let Err(e) = result {
        panic!("property test failed: {e}");
    }
}

// ── Self-check: prove the proptest plumbing propagates failures ──────

#[gpui::test]
#[should_panic(expected = "property test failed")]
fn proptest_self_check(cx: &mut TestAppContext) {
    let config = Config {
        cases: 4,
        max_shrink_iters: 0,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner = TestRunner::new(config);
    let cx_cell: RefCell<&mut TestAppContext> = RefCell::new(cx);

    let result = runner.run(&arb_scenario(), |_scenario| {
        let _cx = cx_cell.borrow_mut();
        Err(TestCaseError::fail(
            "intentional failure from proptest_self_check",
        ))
    });

    if let Err(e) = result {
        panic!("property test failed: {e}");
    }
}
