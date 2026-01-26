//! Transition: change the name of an existing item.
//! Demonstrates a transition that depends on existing state for *both*
//! its strategy (must pick a real id) and its payload (a new name).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransition;
use crate::reference::RefState;
use crate::sut::Sut;
use crate::transition::E2ETransitionFactory;

#[derive(Clone, Debug)]
pub struct RenameItem {
    pub id: u32,
    pub new_name: String,
}

impl E2ETransitionFactory for RenameItem {
    fn weighted_generator(state: &RefState) -> Option<(u32, BoxedStrategy<Self>)> {
        if state.is_empty() {
            return None;
        }
        let ids = state.ids();
        let strat = (proptest::sample::select(ids), "[a-z]{1,8}")
            .prop_map(|(id, new_name)| RenameItem { id, new_name })
            .boxed();
        Some((1, strat))
    }
}

impl E2ETransition for RenameItem {
    fn preconditions(&self, state: &RefState) -> bool {
        state.items.contains_key(&self.id)
    }

    fn apply_to_ref(&self, state: &mut RefState) {
        state.rename(self.id, self.new_name.clone());
    }

    fn apply_to_sut(&self, sut: &mut Sut) {
        let slot = sut.items.get_mut(&self.id).expect("SUT rename: id missing");
        *slot = self.new_name.clone();
    }
}
