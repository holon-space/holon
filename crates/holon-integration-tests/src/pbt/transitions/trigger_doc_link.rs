//! Transition: trigger the [[ doc-link popup and validate the InsertText pipeline.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1083-1100` (generator),
//! `state_machine.rs:3278-3295` (precondition),
//! `state_machine.rs:2547-2550` (ref-state apply — read-only),
//! `sut.rs:3364-3522` (SUT apply), and
//! `transition_budgets.rs:288-294` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE};

use holon_api::{ContentType, EntityUri};

/// Trigger the `[[` doc-link popup on a text block and validate the full
/// `[[ trigger → EditorViewModel → PopupMenu → InsertText` pipeline.
/// Read-only transition: no reference-model state changes.
#[derive(Clone, Debug)]
pub struct TriggerDocLink {
    pub block_id: EntityUri,
    pub target_block_id: EntityUri,
}

impl E2ETransitionFactory for TriggerDocLink {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let all_text: Vec<EntityUri> = state
            .block_state
            .blocks
            .values()
            .filter(|b| b.content_type == ContentType::Text && !b.is_page())
            .map(|b| b.id.clone())
            .collect();

        let count_check: Validated<(), Reason> =
            check(all_text.len() >= 2, Reason::InsufficientTextBlocksForLink);
        if count_check.is_fail() {
            return count_check.map(|_| unreachable!());
        }

        // Enumerate all valid (block_id, target_block_id) pairs by filtering
        // candidates through preconditions. `preconditions` is the single
        // source of truth for validation, so `candidates` may legitimately
        // be empty even when `all_text` has 2+ entries; gate the strategy
        // build below so `select` never sees an empty Vec.
        let candidates: Vec<(EntityUri, EntityUri)> = all_text
            .iter()
            .flat_map(|block_id| {
                all_text.iter().filter_map(move |target_id| {
                    let instance = TriggerDocLink {
                        block_id: block_id.clone(),
                        target_block_id: target_id.clone(),
                    };
                    instance
                        .preconditions(state)
                        .is_good()
                        .then_some((block_id.clone(), target_id.clone()))
                })
            })
            .collect();

        check(
            !candidates.is_empty(),
            Reason::InsufficientTextBlocksForLink,
        )
        .map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|(block_id, target_block_id)| TriggerDocLink {
                    block_id,
                    target_block_id,
                })
                .boxed();
            (1, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for TriggerDocLink {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
        ];

        checks.push(check(
            state.block_state.blocks.contains_key(&self.block_id),
            Reason::FocusedBlockMissing,
        ));
        checks.push(check(
            state.block_state.blocks.contains_key(&self.target_block_id),
            Reason::FocusedBlockMissing,
        ));
        checks.push(check(
            self.block_id != self.target_block_id,
            Reason::PreconditionFailed,
        ));
        checks.push(check(
            state
                .block_state
                .blocks
                .get(&self.block_id)
                .is_some_and(|b| b.content_type == ContentType::Text && !b.is_page()),
            Reason::FocusedNotText,
        ));
        checks.push(check(
            !state.layout_blocks.contains(&self.block_id),
            Reason::FocusedInLayoutBlocks,
        ));
        checks.push(check(
            state.is_descendant_of_any(&self.block_id, &focus_roots),
            Reason::FocusedNotDescendantOfFocusRoot,
        ));
        // Target must also be a text block (and not a page).
        checks.push(check(
            state
                .block_state
                .blocks
                .get(&self.target_block_id)
                .is_some_and(|b| b.content_type == ContentType::Text && !b.is_page()),
            Reason::FocusedNotText,
        ));

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, _: &mut ReferenceState) {
        // Read-only: validates the [[ trigger → InsertText pipeline.
        // No state change in the reference model.
    }

    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_trigger_doc_link(&self.block_id, &self.target_block_id, state)
            .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}
