//! Property-based scenario test combining random widget-tree generation
//! with randomized user-interaction sequences.
//!
//! Each generated `Scenario` carries two things:
//!
//!   - a `Blueprint` — a random `ReactiveViewModel` tree plus a list of
//!     `BlockHandle`s collected during generation (one per mode-switchable
//!     `LiveBlock` in the tree);
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
            StepInput::Mount {
                vm,
                blocks,
                drawer_states,
            } => {
                // Drop any prior session, flush close work, open a new window.
                session = None;
                cx.run_until_parked();
                let s = GpuiScenarioSession::open(*cx, vm, blocks, drawer_states, window_size);
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
    // Budget: 48 cases, aim for ≤20s total.
    //
    // `max_shrink_iters` is capped aggressively: when the oracle fails
    // (e.g. a toggle action that doesn't re-render reaches the shell),
    // proptest spends most of its budget in shrinking. With drawer-heavy
    // scenarios involving 35+ list items and multiple live_blocks, each
    // shrink step costs ~500ms, so 128 iters means minutes of wall-time.
    // 32 iters still produces a readable minimal failure.
    let config = Config {
        cases: 48,
        max_shrink_iters: 32,
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

// ── Targeted: live_block inside tree_item (reproduces ClaudeCode chat view) ──

#[gpui::test]
fn live_block_inside_tree_item_has_nonzero_height(cx: &mut TestAppContext) {
    use holon_layout_testing::generators::arb_tree_with_live_block_items;

    let svg_dir = std::path::PathBuf::from("target/pbt-screenshots/live_block_in_tree");
    std::fs::create_dir_all(&svg_dir).ok();

    let config = Config {
        cases: 5,
        max_shrink_iters: 16,
        ..Config::default()
    };
    let mut runner = TestRunner::new(config);
    let cx_cell: RefCell<&mut TestAppContext> = RefCell::new(cx);
    let case_counter = std::cell::Cell::new(0usize);

    let result = runner.run(&arb_tree_with_live_block_items(), |scenario_bp| {
        let case_idx = case_counter.get();
        case_counter.set(case_idx + 1);

        let mut cx = cx_cell.borrow_mut();
        let scenario = holon_layout_testing::Scenario {
            blueprint: scenario_bp.clone(),
            actions: vec![],
        };

        let cx_cell2: RefCell<&mut TestAppContext> = RefCell::new(*cx);
        let mut session: Option<support::GpuiScenarioSession> = None;

        holon_layout_testing::run_scenario(&scenario, |input| {
            let mut cx = cx_cell2.borrow_mut();
            match input {
                StepInput::Mount {
                    vm,
                    blocks,
                    drawer_states,
                } => {
                    session = None;
                    cx.run_until_parked();
                    let s = support::GpuiScenarioSession::open(
                        *cx,
                        vm,
                        blocks,
                        drawer_states,
                        fixture_window_size(),
                    );
                    let snap = s.snapshot();

                    let svg = snap.to_svg();
                    let path = svg_dir.join(format!("case_{case_idx}.svg"));
                    std::fs::write(&path, &svg).ok();
                    let dump = snap.structural_dump();
                    let dump_path = svg_dir.join(format!("case_{case_idx}.txt"));
                    std::fs::write(&dump_path, &dump).ok();

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
        })?;

        Ok(())
    });

    if let Err(e) = result {
        panic!("live_block in tree_item test failed: {e}");
    }

    eprintln!("SVG diagrams saved to {}", svg_dir.display());
}

// ── Streaming collection data arrival (reproduces production bug) ────
//
// Reproduces the exact production data flow:
// 1. Mount with empty live_blocks (initial_mode=0, empty collection)
// 2. DeliverBlockContent switches to "loaded" mode — but "loaded" is a
//    streaming collection with an EMPTY MutableVec (no data yet)
// 3. GPUI renders → live_block still at zero height (empty collection)
// 4. Push items into the MutableVec (simulating tokio driver data arrival)
// 5. subscribe_inner_collections should detect the VecDiff::Replace and
//    call cx.notify() → parent list re-renders → correct height
//
// If subscribe_inner_collections doesn't fire (or the parent list doesn't
// re-measure), the live_block stays at zero height — reproducing the bug.

#[gpui::test]
fn streaming_collection_data_arrival(cx: &mut TestAppContext) {
    use holon_layout_testing::generators::make_streaming_live_block_fixture;

    let svg_dir = std::path::PathBuf::from("target/pbt-screenshots/streaming");
    std::fs::create_dir_all(&svg_dir).ok();

    let (bp, deferred_data) = make_streaming_live_block_fixture(2, 3);

    let scenario = holon_layout_testing::Scenario {
        blueprint: bp.clone(),
        actions: vec![],
    };

    // Phase A: mount with empty live_blocks
    let session = support::GpuiScenarioSession::open(
        cx,
        scenario.materialize(),
        scenario.block_registrations(),
        std::collections::HashMap::new(),
        fixture_window_size(),
    );
    let snap_a = session.snapshot();
    let dump_a = snap_a.structural_dump();
    std::fs::write(svg_dir.join("A_initial.txt"), &dump_a).ok();
    eprintln!("Phase A (empty):\n{dump_a}");

    // Phase B: deliver structural change (empty → streaming loaded)
    // This switches the live_block to "loaded" mode, but the MutableVec is still empty.
    for h in &bp.handles {
        session.apply_action(
            cx,
            &holon_layout_testing::UiInteraction::DeliverBlockContent {
                block_id: h.block_id.clone(),
            },
        );
    }
    let snap_b = session.snapshot();
    let dump_b = snap_b.structural_dump();
    std::fs::write(svg_dir.join("B_loaded_empty.txt"), &dump_b).ok();
    eprintln!("Phase B (loaded, MutableVec still empty):\n{dump_b}");

    // Check: live_blocks should still be at zero height (empty streaming collection)
    let b_live_blocks: Vec<_> = snap_b
        .entries
        .iter()
        .filter(|(id, _)| id.starts_with("live_block#"))
        .map(|(id, info)| (id.clone(), info.height))
        .collect();
    eprintln!("Phase B live_block heights: {b_live_blocks:?}");

    // Phase C: push data into the MutableVec (simulating tokio driver arrival)
    for (_block_id, mutable_vec, items) in &deferred_data {
        mutable_vec.lock_mut().replace_cloned(items.clone());
    }
    // Let the executor process the VecDiff signal from subscribe_inner_collections
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();
    let snap_c = session.snapshot();
    let dump_c = snap_c.structural_dump();
    std::fs::write(svg_dir.join("C_data_arrived.txt"), &dump_c).ok();
    eprintln!("Phase C (data pushed into MutableVec):\n{dump_c}");

    // Check: live_blocks should now have non-zero height
    let c_live_blocks: Vec<_> = snap_c
        .entries
        .iter()
        .filter(|(id, _)| id.starts_with("live_block#"))
        .map(|(id, info)| (id.clone(), info.height))
        .collect();
    eprintln!("Phase C live_block heights: {c_live_blocks:?}");

    let c_zero: Vec<_> = c_live_blocks.iter().filter(|(_, h)| *h < 1.0).collect();

    assert!(
        c_zero.is_empty(),
        "Phase C: live_blocks still at zero height after data arrival: {c_zero:?}\n\
         Phase B dump:\n{dump_b}\nPhase C dump:\n{dump_c}"
    );

    // Also check that B→C changed (data arrival should expand live_blocks)
    if dump_b == dump_c {
        eprintln!("WARNING: Phase B == Phase C — data arrival had no effect on layout!");
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
