#![cfg(feature = "pbt")]
//! Teeth for `inv-undo-redo-reference-heal`: does an undo→redo round trip
//! restore the referential integrity that held before the undo?
//!
//! The reference site driven here is the open `navigation_history` row a
//! `PinBlock` writes. Pins are not undoable (`PinBlock::apply_to_ref` records
//! no undo snapshot), so the pin OUTLIVES the undo of the split that minted its
//! target — exactly the shape the invariant is built to observe.
//!
//! Timeline of the round trip:
//!   SplitBlock(c1, 0)  mints tail U1, oracle label `block::split-0`.
//!   PinBlock(U1)       writes navigation_history(open) -> U1.
//!   Undo               DELETES U1. The pin now dangles — CORRECT: the block is
//!                      genuinely gone. The redo-gated burned set is still
//!                      EMPTY, so the invariant says nothing. That is phase 1.
//!   Redo               re-executes the forward op and mints a FRESH uuid U2.
//!                      The reconcile retires U1 into `redo_burned` on this
//!                      exact tick: the block is back under a new identity and
//!                      healing was due. The pin still names U1. That is
//!                      phase 2.
//!
//! ## Why this asserts on the MESSAGE, not on "a panic happened"
//!
//! `inv-focus-roots` ALSO fires on the phase-2 shape (the oracle resolves the
//! pin's synthetic label through the re-pointed reconcile map and predicts the
//! healed id). So "the sequence panics" is satisfied by a completely DEAD
//! `inv-undo-redo-reference-heal` — an adversarial review neutered the body
//! with an early `return InvariantResult::Ok` and a panic-only assertion still
//! passed. The assertions below therefore require the panic text to name this
//! invariant AND the unhealed reference site it diagnosed.

use std::panic::AssertUnwindSafe;

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::invariants::bodies::undo_redo_reference_heal::ENFORCE_ENV;
use holon_integration_tests::pbt::invariants::bodies::undo_redo_reference_heal::ENFORCE_VALUE;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_pbt_core::fixture::Fixture;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

/// `SplitBlock(c1,0)` then pin the minted tail — the reference that must
/// survive the undo. Phase 1 stops here; phase 2 appends the `Redo`.
const BARE_UNDO: &str = r#"{"name": "pin-split-tail-bare-undo", "description": "a pin dangling after an undo is CORRECT — the invariant must stay silent", "transitions": [{"SplitBlock": {"block_id": "block:c1", "position": 0}}, {"PinBlock": {"region": "right_sidebar", "block_id": "block::split-0"}}, {"UndoLastMutation": null}]}"#;

const ROUND_TRIP: &str = r#"{"name": "pin-split-tail-undo-redo", "description": "a pin written against the split tail's pre-undo identity is NOT healed by the redo", "transitions": [{"SplitBlock": {"block_id": "block:c1", "position": 0}}, {"PinBlock": {"region": "right_sidebar", "block_id": "block::split-0"}}, {"UndoLastMutation": null}, {"Redo": null}]}"#;

fn replay(line: &str) {
    let case: Fixture<E2ETransition> = serde_json::from_str(line).expect("teeth case must parse");
    let config = Config {
        verbose: 1,
        ..Config::default()
    };
    let initial_state = wide_e2e_ref();
    ComposedSut::<WideE2E>::test_sequential(config, initial_state, case.transitions, None);
}

/// The panic text, or a loud failure if the payload is neither `&str` nor
/// `String` — an unreadable payload would let the substring assertions below
/// pass vacuously.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    panic!("panic payload is neither &str nor String — cannot verify the failure message");
}

/// Both halves of the teeth, in ONE test so the process-global enforce env is
/// set exactly once and the two phases cannot race.
///
/// Phase 1 (negative): the bare undo must be GREEN under enforce. A "no
/// dangling references" formulation would fail here — the pin genuinely dangles
/// at that point, and reporting it would be a false alarm.
///
/// Phase 2 (positive): the completed round trip must fail, and the failure must
/// be ATTRIBUTED to this invariant with the reference site named.
///
/// If prod is ever changed so a redo re-points references, phase 2 starts
/// failing — that is the correct signal to revisit this test and the
/// 2026-07-25 ruling, not to weaken the assertion.
#[test]
fn undo_redo_reference_heal_teeth() {
    // SAFETY: this binary has exactly one test, so nothing else reads or writes
    // the environment concurrently.
    unsafe { std::env::set_var(ENFORCE_ENV, ENFORCE_VALUE) };

    eprintln!("=== phase 1: bare undo must leave the invariant SILENT (under enforce) ===");
    replay(BARE_UNDO);

    eprintln!("=== phase 2: completed round trip must be RED, attributed to this invariant ===");
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| replay(ROUND_TRIP)));
    let payload = outcome.err().unwrap_or_else(|| {
        panic!(
            "phase 2 PASSED, but the undo→redo round trip must fail under \
             {ENFORCE_ENV}={ENFORCE_VALUE}: the pin still names the burned id"
        )
    });
    let msg = panic_text(payload.as_ref());

    // The neuter test: with the body dead, `inv-focus-roots` still panics on
    // this shape, so every one of these substrings must be required.
    for needle in [
        "inv-undo-redo-reference-heal",
        "undo→redo did NOT restore referential integrity",
        "navigation_history(open).block_id",
        "burned ids:",
    ] {
        assert!(
            msg.contains(needle),
            "phase 2 failed, but NOT with this invariant's diagnosis — missing {needle:?}. A \
             panic alone proves nothing here (inv-focus-roots fires on the same shape).\n  full \
             panic text:\n{msg}"
        );
    }
    eprintln!("=== phase 2 RED, correctly attributed ===\n{msg}");
}
