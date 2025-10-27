//! Transition: navigate to a specific block within a region.
//!
//! Mirrors the legacy logic split across `state_machine.rs:568-601` (generator),
//! `state_machine.rs:3165-3167` (precondition),
//! `state_machine.rs:2222-2241` (ref-state apply),
//! `sut.rs:1266-1292` (SUT apply), and
//! `transition_budgets.rs:165-172` (expected SQL).

use holon_api::EntityUri;
use holon_api::Region;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::NavigationHistory;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

/// Navigate to focus on a specific block within a region.
#[derive(Clone, Debug)]
pub struct NavigateFocus {
    pub region: Region,
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for NavigateFocus {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }

        // Restricted to Main: in production the only UI that triggers
        // `navigation.focus` is the LeftSidebar selectable's bound
        // action, and it ALWAYS targets `region: "main"`. There is no
        // user-facing way to push history into the LeftSidebar's or
        // RightSidebar's own region. Earlier versions of this generator
        // emitted those regions and the SUT covered them with a
        // `execute_op + manual set_focus` shortcut — that's the API
        // shortcut item A2 in `frontends/tui/TODO.md` removes.
        //
        // Targets are restricted to LeftSidebar-listed pages
        // (`focusable_rendered_block_ids(LeftSidebar)`), which is what
        // the user can actually click in the sidebar.
        let navigable_block_ids = state.focusable_rendered_block_ids(Region::LeftSidebar);

        if navigable_block_ids.is_empty() {
            return None;
        }

        let strat = prop::sample::select(navigable_block_ids)
            .prop_map(|block_id| NavigateFocus {
                region: Region::Main,
                block_id,
            })
            .boxed();

        Some((3, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for NavigateFocus {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        state.app_started && state.block_state.blocks.contains_key(&self.block_id)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let history = state
            .navigation_history
            .entry(self.region)
            .or_insert_with(NavigationHistory::new);

        history.entries.truncate(history.cursor + 1);
        history.entries.push(Some(self.block_id.clone()));
        history.cursor = history.entries.len() - 1;

        // NavigateFocus changes what's displayed but clears editor focus —
        // the previously-focused block may no longer be visible.
        state.focused_entity_id.remove(&self.region);
        state.focused_cursor.remove(&self.region);

        // Mirror `UiState::set_focus`: the navigation target becomes the
        // globally focused block. `focus_chain()` and `chain_ops()` read
        // from this — inv11 asserts they reflect the predicted URI.
        state.focused_block = Some(self.block_id.clone());
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_navigate_focus(self.region, &self.block_id).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}
