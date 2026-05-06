//! Transition: outdent the focused block (move it up to its grandparent level).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1122-1137` (generator),
//! `state_machine.rs:3329-3343` (precondition),
//! `state_machine.rs:2662-2665` (ref-state apply),
//! `sut.rs:3548-3553` (SUT apply), and
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

/// Outdent the focused block: move it up one level to its grandparent via the
/// `Alt+Left` / Shift+Tab chord.
#[derive(Clone, Debug)]
pub struct Outdent {
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for Outdent {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Candidate set: Main's editable descendants. Per-precondition
        // filter narrows to outdentable subset (grandparent exists).
        let candidates: Vec<EntityUri> = state
            .main_editable_descendants()
            .into_iter()
            .filter(|uri| {
                Outdent {
                    block_id: uri.clone(),
                }
                .preconditions(state)
                .is_good()
            })
            .collect();
        check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|block_id| Outdent { block_id })
                .boxed();
            (1, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for Outdent {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
        ];

        let block = state.block_state.blocks.get(&self.block_id);
        checks.push(check(block.is_some(), Reason::FocusedBlockMissing));
        if let Some(b) = block {
            checks.push(check(
                b.content_type == ContentType::Text,
                Reason::FocusedNotText,
            ));
        }

        checks.push(check(
            !state.layout_blocks.contains(&self.block_id),
            Reason::FocusedInLayoutBlocks,
        ));
        checks.push(check(
            state.is_descendant_of_any(&self.block_id, &focus_roots),
            Reason::FocusedNotDescendantOfFocusRoot,
        ));
        checks.push(check(
            state.grandparent(&self.block_id).is_some(),
            Reason::NoGrandparent,
        ));

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.push_undo_snapshot();
        state.outdent_block(&self.block_id);
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_outdent(&self.block_id).await;
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
