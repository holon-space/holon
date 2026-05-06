//! Transition: move the focused block up (swap with its previous sibling).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1138-1153` (generator),
//! `state_machine.rs:3344-3358` (precondition),
//! `state_machine.rs:2667-2671` (ref-state apply),
//! `sut.rs:3555-3560` (SUT apply), and
//! `transition_budgets.rs:298-302` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

use holon_api::{ContentType, EntityUri};

/// Move the focused block up: swap its sort_key with its previous sibling's.
#[derive(Clone, Debug)]
pub struct MoveUp {
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for MoveUp {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Single-instance pattern: focused block is the only candidate.
        // Delegate all validation to preconditions.
        let Some(block_id) = state.focused_entity(holon_api::Region::Main).cloned() else {
            return Validated::fail(Reason::NoFocusInMain);
        };
        let instance = MoveUp { block_id };
        instance.preconditions(state).map(|()| {
            let strat = Just(instance.clone()).boxed();
            (1, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for MoveUp {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
        ];

        let focus = state.focused_entity(holon_api::Region::Main);
        checks.push(check(
            focus == Some(&self.block_id),
            Reason::FocusedIsNotSelf,
        ));

        if focus == Some(&self.block_id) {
            let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
            let no_content_update: std::collections::HashSet<EntityUri> = state
                .layout_blocks
                .render_source_ids
                .iter()
                .chain(state.layout_blocks.query_source_ids.iter())
                .chain(state.profile_block_ids.iter())
                .cloned()
                .collect();

            let block = state.block_state.blocks.get(&self.block_id);
            checks.push(check(
                block.is_some_and(|b| b.content_type == ContentType::Text),
                Reason::FocusedNotText,
            ));
            if let Some(b) = block {
                checks.push(check(!b.is_page(), Reason::FocusedIsPage));
            }
            checks.push(check(
                state.layout_blocks.is_focusable(&self.block_id),
                Reason::FocusedNotFocusable,
            ));
            checks.push(check(
                !no_content_update.contains(&self.block_id),
                Reason::FocusedInNoContentUpdate,
            ));
            checks.push(check(
                state.is_descendant_of_any(&self.block_id, &focus_roots),
                Reason::FocusedNotDescendantOfFocusRoot,
            ));
            checks.push(check(
                state.previous_sibling(&self.block_id).is_some(),
                Reason::NoPreviousSibling,
            ));
        }

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.push_undo_snapshot();
        let prev_id = state.previous_sibling(&self.block_id).unwrap();
        state.swap_sequence(&self.block_id, &prev_id);
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_move_up(&self.block_id).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let mut sql = expected_sql_for_kind(
            MutationKind::Update,
            state.active_watches.len(),
            state.block_state.blocks.len(),
            state.documents.len(),
        );
        sql.tolerance += 5; // extra margin for ordering operations
        sql
    }
}
