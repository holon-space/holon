//! End-to-end PBT wiring. The interesting bit is how *little* code is
//! needed here — the per-transition logic lives entirely in
//! `src/transitions/<name>.rs`.

use pbt_state_machine::reference::RefState;
use pbt_state_machine::sut::Sut;
use pbt_state_machine::transitions::E2ETransition;
use pbt_state_machine::transitions::E2ETransitionEnum;
use pbt_state_machine::transitions::transitions;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

struct ItemsRef;

impl ReferenceStateMachine for ItemsRef {
    type State = RefState;
    type Transition = E2ETransitionEnum;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(RefState::default()).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        // The macro-generated function — only line referencing each
        // variant. Adding/removing transitions never touches this file.
        transitions(state)
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        transition.apply_to_ref(&mut state);
        state
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        transition.preconditions(state)
    }
}

struct ItemsTest;

impl StateMachineTest for ItemsTest {
    type SystemUnderTest = Sut;
    type Reference = ItemsRef;

    fn init_test(
        _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        Sut::default()
    }

    fn apply(
        mut sut: Self::SystemUnderTest,
        _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        transition.apply_to_sut(&mut sut);
        sut
    }

    fn check_invariants(
        sut: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        assert_eq!(
            sut.items, ref_state.items,
            "SUT diverged from reference state"
        );
    }
}

prop_state_machine! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        .. ProptestConfig::default()
    })]

    #[test]
    fn ref_and_sut_stay_in_sync(sequential 1..32 => ItemsTest);
}
