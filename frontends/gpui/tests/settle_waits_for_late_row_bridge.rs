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

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::Value;
use holon_gpui::bridge_hold::hold_row_bridge;
use holon_integration_tests::pbt::ReferenceState;
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

use crate::pbt_harness::panic_message;
use crate::pbt_harness::windowed_wide::SETTLE_DEADLINE;
use crate::pbt_harness::windowed_wide::with_windowed_wide_sut;
use crate::pbt_harness::windowed_wide::with_windowed_wide_sut_live;

/// The settle deadline while a bridge is held: the settle must time out on
/// the held row, and this bounds how long each case waits for that.
const HELD_SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// One late-bridge case: a failure on the first divergence, a rejection when
/// the insert never reached the held bridge.
fn late_bridge_case(
    initial: &ReferenceState,
    parent: EntityUri,
    content: String,
) -> Result<(), TestCaseError> {
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
    let out: RefCell<Result<(), TestCaseError>> = RefCell::new(Ok(()));
    with_windowed_wide_sut_live(|sut, _default_oracle, sink, live| {
        if let Err(p) = catch_unwind(AssertUnwindSafe(|| {
            ComposedSut::<WideE2E>::check_invariants(&sut, initial)
        })) {
            *out.borrow_mut() = Err(TestCaseError::fail(format!("boot: {}", panic_message(&p))));
            return Some(sut);
        }
        let transition = E2ETransition::ApplyMutation(write.clone());
        let ref_state =
            <WideE2EMachine as ReferenceStateMachine>::apply(initial.clone(), &transition);

        let hold = hold_row_bridge(row.clone());
        let sut = match catch_unwind(AssertUnwindSafe(|| {
            ComposedSut::<WideE2E>::apply(sut, &ref_state, transition)
        })) {
            Ok(sut) => sut,
            Err(p) => {
                *out.borrow_mut() = Err(TestCaseError::fail(format!(
                    "SUT apply panicked: {}",
                    panic_message(&p)
                )));
                return None;
            }
        };
        live.set_settle_deadline(HELD_SETTLE_DEADLINE);
        let held = catch_unwind(AssertUnwindSafe(|| {
            ComposedSut::<WideE2E>::check_invariants(&sut, &ref_state);
        }));
        live.set_settle_deadline(SETTLE_DEADLINE);
        // Some inserts paint without passing a row bridge; such a case cannot
        // hold a row late, and the reject limit still fails a generator that
        // stops reaching the bridge.
        if !hold.engaged() {
            hold.open();
            *out.borrow_mut() = Err(TestCaseError::reject(format!(
                "{write:?} never reached the held bridge of {row}"
            )));
            return Some(sut);
        }
        let verdict = match held {
            Ok(()) => Some(format!(
                "the settle returned while the bridge of {row} was held (painted: {})",
                live.painted(&row)
            )),
            Err(p) => {
                let msg = panic_message(&p);
                match msg.contains("window did not settle") && msg.contains(row.as_str()) {
                    true => None,
                    false => Some(format!(
                        "the held settle failed, but not by timing out on {row}: {msg}"
                    )),
                }
            }
        };
        hold.open();
        ComposedSut::<WideE2E>::check_invariants(&sut, &ref_state);
        assert!(
            live.painted(&row),
            "{write:?}: the settle returned after the bridge opened, but {row} is not painted"
        );
        if let Some(msg) = verdict {
            let msg = format!("{write:?} with a late bridge: {msg}");
            sink.capture_fail("ApplyMutation", &msg);
            *out.borrow_mut() = Err(TestCaseError::fail(msg));
        }
        Some(sut)
    });
    out.into_inner()
}

#[test]
fn settle_waits_for_a_late_row_bridge() {
    let mut initial = None;
    with_windowed_wide_sut(|sut, _default_oracle, _sink| {
        initial = Some(wide_e2e_windowed_ref(narrow_to_windowed_alphabet(
            sut.cap_set(),
        )));
        Some(sut)
    });
    let initial = initial.expect("the boot ran");

    let examples = [(EntityUri::block("parent"), "late bridge".to_string())];
    for (parent, content) in examples {
        if let Err(e) = late_bridge_case(&initial, parent, content) {
            panic!("[late-bridge] static example: {e}");
        }
    }

    let rows = initial.rendered_state_toggle_ids();
    assert!(!rows.is_empty(), "the booted window shows no block rows");
    let strategy = (prop::sample::select(rows), "[a-z]{3,8}( [a-z]{3,8})?");
    let cases: u32 = std::env::var("PROPTEST_CASES")
        .ok()
        .map(|s| s.parse().expect("PROPTEST_CASES is a number"))
        .unwrap_or(4);
    let mut runner = TestRunner::new(Config {
        cases,
        max_shrink_iters: 4,
        ..Config::default()
    });
    let rejected = std::cell::Cell::new(0u32);
    let result = runner.run(&strategy, |(parent, content)| {
        let verdict = late_bridge_case(&initial, parent, content);
        if let Err(TestCaseError::Reject(_)) = verdict {
            rejected.set(rejected.get() + 1);
        }
        verdict
    });
    eprintln!("[late-bridge] rejected cases: {}", rejected.get());
    if let Err(e) = result {
        panic!("[late-bridge] {e}");
    }
}
