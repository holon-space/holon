//! Fakes shared by the hand-authored replay test binaries.

/// Counts transitions applied; stands in for the keystone reference.
pub struct CountingRef;

impl proptest_state_machine::ReferenceStateMachine for CountingRef {
    type State = usize;
    type Transition = ();

    fn init_state() -> proptest::strategy::BoxedStrategy<usize> {
        proptest::strategy::Strategy::boxed(proptest::strategy::Just(0))
    }

    fn transitions(_: &usize) -> proptest::strategy::BoxedStrategy<()> {
        proptest::strategy::Strategy::boxed(proptest::strategy::Just(()))
    }

    fn apply(applied: usize, _: &()) -> usize {
        applied + 1
    }
}

pub fn four_transition_case(extra: &str) -> String {
    let create = r#"{"CreateBlockUnderFocus": {"content": "c", "id": "block:c"}}"#;
    format!(
        r#"{{"name": "n", "locks": ["inv-x"], "transitions": [{create}, {create}, {create}, {create}]{extra}}}"#
    )
}
