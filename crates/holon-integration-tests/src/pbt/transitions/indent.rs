//! Transition: indent the focused block (make it a child of its previous sibling).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1106-1121` (generator),
//! `state_machine.rs:3314-3328` (precondition),
//! `state_machine.rs:2648-2660` (ref-state apply),
//! `sut.rs:3541-3546` (SUT apply), and
//! `transition_budgets.rs:298-302` (expected SQL).

use holon_api::{ContentType, EntityUri};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

/// Indent the focused block: re-parent it under its previous sibling via the
/// `Alt+Right` / Tab chord.
#[derive(Clone, Debug)]
pub struct Indent {
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for Indent {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Candidate set: Main's editable descendants (text, non-page,
        // non-layout, non-locked, descendant of Main's focus_roots).
        // Per-precondition filter narrows it to indentable subset
        // (previous-sibling exists).
        let candidates: Vec<EntityUri> = state
            .main_editable_descendants()
            .into_iter()
            .filter(|uri| {
                Indent {
                    block_id: uri.clone(),
                }
                .preconditions(state)
                .is_good()
            })
            .collect();
        check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|block_id| Indent { block_id })
                .boxed();
            (1, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for Indent {
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
            state.previous_sibling(&self.block_id).is_some(),
            Reason::NoPreviousSibling,
        ));

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.push_undo_snapshot();
        let prev_id = state.previous_sibling(&self.block_id).unwrap();
        // Production indent re-parents the block under its previous
        // sibling, anchored after that parent's current last child —
        // i.e. it lands at the end of the new sibling group. Mirror
        // that with `move_block(after = last_child_of(prev_id))`.
        let after = state
            .sorted_children_of(&prev_id)
            .last()
            .map(|b| b.id.clone());
        state.move_block(&self.block_id, prev_id, after.as_ref());
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_indent(&self.block_id).await;
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
