//! A late row bridge: the engine created a row under a visible row, but the
//! window's list did not take the insert yet. Settling the window must not
//! return before a frame paints the row: while the bridge is held it fails
//! loud naming the row, and once it opens it returns with the row painted.
//!
//! The defect class is a block created under any task row of the booted main
//! panel, with the new row's bridges held shut across the step's settle and
//! checks. A backend write, not a gesture: a gesture driver waits for the
//! window between clicks, which a held bridge never lets it see. A content
//! write is left out: a row's text reads the shared row cell at paint, so a
//! held bridge cannot keep old text on screen.

use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::Value;
use holon_gpui::bridge_hold::hold_row_bridge;
use holon_gpui::prewarm_seam::time_out_next_prewarm;
use holon_integration_tests::pbt::ReferenceState;
use holon_integration_tests::pbt::composed::case_budget::cases_or;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::narrow_to_windowed_alphabet;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_windowed_ref;
use holon_integration_tests::pbt::transitions::ApplyMutation;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefTaskStateToggle;
use holon_pbt_core::types::Mutation;
use holon_pbt_core::types::MutationEvent;
use holon_pbt_core::types::MutationSource;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

use crate::pbt_harness::capture::FrameSink;
use crate::pbt_harness::panic_message;
use crate::pbt_harness::windowed_wide::LivePanels;
use crate::pbt_harness::windowed_wide::SETTLE_DEADLINE;
use crate::pbt_harness::windowed_wide::with_windowed_wide_sut_live;

/// The settle deadline while a bridge is held: the settle must time out on
/// the held row, and this bounds how long each case waits for that.
const HELD_SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// One late-bridge case on a fresh window, opened on the loading state when
/// `prewarm_timed_out`, and the reference state it booted: a failure on the
/// first divergence, also when the write never reached the bridge held
/// before it.
fn late_bridge_case(
    parent: EntityUri,
    content: String,
    prewarm_timed_out: bool,
) -> (ReferenceState, Result<(), TestCaseError>) {
    if prewarm_timed_out {
        time_out_next_prewarm();
    }
    let mut booted = None;
    let mut out = Ok(());
    with_windowed_wide_sut_live(|sut, _default_oracle, sink, live| {
        let initial = booted.insert(wide_e2e_windowed_ref(narrow_to_windowed_alphabet(
            sut.cap_set(),
        )));
        let (sut, verdict) = held_write(sut, initial, parent, content, sink, live);
        out = verdict;
        sut
    });
    (booted.expect("the boot ran"), out)
}

fn held_write(
    sut: ComposedSut<WideE2E>,
    initial: &ReferenceState,
    parent: EntityUri,
    content: String,
    sink: &FrameSink,
    live: &LivePanels,
) -> (Option<ComposedSut<WideE2E>>, Result<(), TestCaseError>) {
    let row = EntityUri::block(&format!("block-{}", initial.next_block_id()));
    let write = ApplyMutation {
        event: MutationEvent {
            source: MutationSource::External,
            mutation: Mutation::Create {
                id: row.clone(),
                parent_id: parent,
                fields: [
                    ("content".to_string(), Value::String(content)),
                    ("content_type".to_string(), ContentType::Text.into()),
                ]
                .into_iter()
                .collect(),
            },
        },
    };
    assert!(
        write.preconditions(initial).is_good(),
        "{write:?} is not a legal step on the booted state"
    );
    if let Err(p) = catch_unwind(AssertUnwindSafe(|| {
        ComposedSut::<WideE2E>::check_invariants(&sut, initial)
    })) {
        return (
            Some(sut),
            Err(TestCaseError::fail(format!("boot: {}", panic_message(&p)))),
        );
    }
    {
        let transition = E2ETransition::ApplyMutation(write.clone());
        let ref_state =
            <WideE2EMachine as ReferenceStateMachine>::apply(initial.clone(), &transition);

        let hold = hold_row_bridge(row.clone());
        let sut = match catch_unwind(AssertUnwindSafe(|| {
            ComposedSut::<WideE2E>::apply(sut, &ref_state, transition)
        })) {
            Ok(sut) => sut,
            Err(p) => {
                return (
                    None,
                    Err(TestCaseError::fail(format!(
                        "SUT apply panicked: {}",
                        panic_message(&p)
                    ))),
                );
            }
        };
        live.set_settle_deadline(HELD_SETTLE_DEADLINE);
        let held = catch_unwind(AssertUnwindSafe(|| {
            ComposedSut::<WideE2E>::check_invariants(&sut, &ref_state);
        }));
        live.set_settle_deadline(SETTLE_DEADLINE);
        if !hold.engaged() {
            let painted = live.painted(&row);
            hold.open();
            let why = match (&held, painted) {
                (_, true) => format!("{row} painted without passing its row bridge"),
                (Ok(()), false) => format!(
                    "the settle returned before {row} reached its row bridge or a painted frame"
                ),
                (Err(p), false) => format!(
                    "the held settle failed before {row} reached its row bridge: {}",
                    panic_message(p)
                ),
            };
            let msg = format!("{write:?} never reached the held bridge: {why}");
            sink.capture_fail("ApplyMutation", &msg);
            return (Some(sut), Err(TestCaseError::fail(msg)));
        }
        let verdict = match held {
            Ok(()) => Some(format!(
                "the settle returned while the bridge of {row} was held (painted: {})",
                live.painted(&row)
            )),
            Err(p) => {
                let msg = panic_message(&p);
                match (
                    msg.contains("window did not settle") && msg.contains(row.as_str()),
                    live.take_settle_timeout(),
                ) {
                    (true, Some(t)) if t.runnable => Some(format!(
                        "the held settle timed out on {row} saying a task is still runnable, \
                         yet a held bridge waits on a signal and no task wakes itself"
                    )),
                    (true, Some(t)) if t.epoch.reflects < t.target => None,
                    (true, Some(t)) => Some(format!(
                        "the held settle timed out on {row} with a frame that reflects {:?}, \
                         not below the target {:?}",
                        t.epoch.reflects, t.target
                    )),
                    (true, None) => Some(format!(
                        "the held settle timed out on {row} but recorded no timeout"
                    )),
                    (false, _) => Some(format!(
                        "the held settle failed, but not by timing out on {row}: {msg}"
                    )),
                }
            }
        };
        hold.open();
        if let Some(msg) = verdict {
            let msg = format!("{write:?} with a held bridge: {msg}");
            sink.capture_fail("ApplyMutation", &msg);
            return (Some(sut), Err(TestCaseError::fail(msg)));
        }
        ComposedSut::<WideE2E>::check_invariants(&sut, &ref_state);
        assert!(
            live.painted(&row),
            "{write:?}: the settle returned after the bridge opened, but {row} is not painted"
        );
        (Some(sut), Ok(()))
    }
}

#[test]
fn settle_waits_for_a_late_row_bridge() {
    let parent = EntityUri::block("parent");
    let mut initial = None;
    for prewarm_timed_out in [true, false] {
        let (booted, verdict) =
            late_bridge_case(parent.clone(), "late bridge".to_string(), prewarm_timed_out);
        if let Err(e) = verdict {
            panic!("[late-bridge] static example, pre-warm timed out {prewarm_timed_out}: {e}");
        }
        initial = Some(booted);
    }
    let initial = initial.expect("a static example ran");

    let rows = initial.rendered_state_toggle_ids();
    assert!(!rows.is_empty(), "the booted window shows no block rows");
    let strategy = (
        prop::sample::select(rows),
        "[a-z]{3,8}( [a-z]{3,8})?",
        any::<bool>(),
    );
    // Each case boots a window: the static examples and the cases stay under
    // the test's nextest cap at load.
    let cases: u32 = cases_or("PROPTEST_CASES", 2);
    let mut runner = TestRunner::new(Config {
        cases,
        max_shrink_iters: 4,
        ..Config::default()
    });
    let result = runner.run(&strategy, |(parent, content, prewarm_timed_out)| {
        late_bridge_case(parent, content, prewarm_timed_out).1
    });
    if let Err(e) = result {
        panic!("[late-bridge] {e}");
    }
}
