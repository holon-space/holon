//! Transition: remove an existing item by id.
//! Conditionally enabled — covers the `enabled = None` skip case.

use super::E2ETransition;
use crate::reference::RefState;
use crate::sut::Sut;
use crate::transition::E2ETransitionFactory;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

#[derive(Clone, Debug)]
pub struct RemoveItem {
    pub id: u32,
}

impl E2ETransitionFactory for RemoveItem {
    fn weighted_generator(state: &RefState) -> Option<(u32, BoxedStrategy<Self>)> {
        if state.is_empty() {
            return None;
        }
        let ids = state.ids();
        let strat = proptest::sample::select(ids)
            .prop_map(|id| RemoveItem { id })
            .boxed();
        Some((2, strat))
    }
}

impl E2ETransition for RemoveItem {
    fn preconditions(&self, state: &RefState) -> bool {
        state.items.contains_key(&self.id)
    }

    fn apply_to_ref(&self, state: &mut RefState) {
        state.remove(self.id);
    }

    fn apply_to_sut(&self, sut: &mut Sut) {
        let prev = sut.items.remove(&self.id);
        assert!(prev.is_some(), "SUT remove: id {} missing", self.id);
    }
}
