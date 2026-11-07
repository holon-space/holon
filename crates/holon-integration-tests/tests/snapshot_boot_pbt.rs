#![cfg(feature = "pbt")]
//! A keystone case booted from its key's template must be indistinguishable
//! from a fresh boot, whatever ran in the process before it.

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::snapshot_boot::BootKey;
use holon_integration_tests::pbt::composed::snapshot_boot::assert_obs_eq;
use holon_integration_tests::pbt::composed::snapshot_boot::observe;
use holon_integration_tests::pbt::composed::snapshot_boot::template_for;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::set_for_wiring;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::composed::wide_e2e::wide_init_state;
use holon_integration_tests::pbt::hand_authored::HandAuthoredCase;
use holon_integration_tests::pbt::hand_authored::load_cases;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_pbt_core::BootFault;
use holon_pbt_core::Projection;
use holon_pbt_core::capabilities::IntegrationConnectTiming;
use proptest::prelude::Just;
use proptest::strategy::Strategy;
use proptest::test_runner::Config;
use proptest::test_runner::TestRunner;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

/// A template holds a finished session store, so only a wiring that boots a
/// session has one; a boot fault or a deferred connect changes the boot itself.
fn restorable(state: &ReferenceState) -> bool {
    set_for_wiring(&state.harness.wiring).has_projection(Projection::ViewModel)
        && state.boot_fault == BootFault::None
        && state.integration.timing == IntegrationConnectTiming::Instant
}

fn restorable_init_state() -> impl Strategy<Value = ReferenceState> {
    wide_init_state(
        Just(IntegrationConnectTiming::Instant).boxed(),
        Just(BootFault::None).boxed(),
    )
    .prop_filter("a session boot", restorable)
}

fn sidecar_initial_state(case: &HandAuthoredCase) -> ReferenceState {
    match &case.initial_state {
        Some(pinned) => pinned
            .clone()
            .into_reference_state(&format!("snapshot boot example {:?}", case.name)),
        None => wide_e2e_ref(),
    }
}

/// One state per distinct key among the hand-authored cases.
fn sidecar_states() -> Vec<ReferenceState> {
    let mut seen = std::collections::HashSet::new();
    load_cases()
        .iter()
        .map(sidecar_initial_state)
        .filter(|state| seen.insert(BootKey::of(state)))
        .collect()
}

fn runner() -> TestRunner {
    TestRunner::new(Config {
        cases: 8,
        failure_persistence: None,
        ..Config::default()
    })
}

fn restore_matches_fresh(state: &ReferenceState) {
    let fresh = ComposedSut::<WideE2E>::init_test(state);
    let fresh_obs = observe(&fresh);
    drop(fresh);
    let template = template_for(state);
    let restored = ComposedSut::<WideE2E>::init_restored(&template, state);
    assert_obs_eq(
        &fresh_obs,
        &observe(&restored),
        &format!("restore of {} vs a fresh boot", BootKey::of(state)),
    );
}

#[test]
fn a_restored_boot_observes_what_a_fresh_boot_observes() {
    for state in sidecar_states().iter().filter(|s| restorable(s)) {
        restore_matches_fresh(state);
    }
    runner()
        .run(&restorable_init_state(), |state| {
            restore_matches_fresh(&state);
            Ok(())
        })
        .unwrap();
}

/// Runs `transitions` on a restored SUT, then checks that a second restore of
/// the same key still observes the template.
fn restore_after(init: &ReferenceState, transitions: &[E2ETransition]) {
    let template = template_for(init);
    let mut ref_state = init.clone();
    let mut sut = ComposedSut::<WideE2E>::init_restored(&template, init);
    for transition in transitions {
        ref_state = WideE2EMachine::apply(ref_state, transition);
        sut = ComposedSut::<WideE2E>::apply(sut, &ref_state, transition.clone());
    }
    drop(sut);
    let again = ComposedSut::<WideE2E>::init_restored(&template, init);
    assert_obs_eq(
        template.obs(),
        &observe(&again),
        &format!(
            "restore of {} after {} transition(s)",
            template.key(),
            transitions.len()
        ),
    );
}

fn first_case_with(pick: fn(&E2ETransition) -> bool) -> HandAuthoredCase {
    load_cases()
        .into_iter()
        .find(|case| case.transitions.iter().any(pick))
        .expect("the sidecar holds a case with this transition")
}

#[test]
fn a_restore_does_not_see_the_cases_before_it() {
    for case in [
        first_case_with(|t| matches!(t, E2ETransition::CrashRestart(_))),
        first_case_with(|t| matches!(t, E2ETransition::RebootWhileDatabaseHeld(_))),
    ] {
        restore_after(&sidecar_initial_state(&case), &case.transitions);
    }
    runner()
        .run(
            &WideE2EMachine::sequential_strategy(1..6)
                .prop_filter("a session boot", |(init, _, _)| restorable(init)),
            |(init, transitions, _)| {
                restore_after(&init, &transitions);
                Ok(())
            },
        )
        .unwrap();
}
