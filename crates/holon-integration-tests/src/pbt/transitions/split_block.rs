//! Transition: split a block at a byte position.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1171-1191` (generator),
//! `state_machine.rs:3426-3436` (precondition),
//! `state_machine.rs:2687-2691` (ref-state apply),
//! `sut.rs:4052-4127` (SUT apply), and
//! `transition_budgets.rs:303-312` (expected SQL).

use holon_api::ContentType;
use holon_api::Region;
use holon_api::entity_uri::EntityUri;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::{CursorPosition, ReferenceState};
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, MutationKind, REACTIVE_BASE, expected_sql_for_kind,
};

/// Split an editable text block at a byte position.
/// Only the currently focused (editable) block is a candidate.
#[derive(Clone, Debug)]
pub struct SplitBlock {
    pub block_id: EntityUri,
    pub position: usize,
}

impl E2ETransitionFactory for SplitBlock {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // SplitBlock fires on Enter against the active editor in production —
        // any text descendant of Main's focus_roots is a legal target.
        // Content is ASCII-only in PBT, so byte == char.
        // Candidate set = Main's editable descendants; per-position
        // preconditions filter to valid positions (0..=content_len).
        let candidates: Vec<(EntityUri, usize)> = {
            let editable_block_ids = state.main_editable_descendants();
            let mut result = vec![];
            for block_id in editable_block_ids {
                if let Some(block) = state.block_state.blocks.get(&block_id) {
                    let content_len = block.content_text().len();
                    for position in 0..=content_len {
                        if (SplitBlock {
                            block_id: block_id.clone(),
                            position,
                        })
                        .preconditions(state)
                        .is_good()
                        {
                            result.push((block_id.clone(), position));
                        }
                    }
                }
            }
            result
        };
        check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|(block_id, position)| SplitBlock { block_id, position })
                .boxed();
            // High weight when eligible: editing transitions are starved
            // unless Main is navigated to a populated doc, so when we have
            // editable descendants we want SplitBlock to fire often enough
            // to exercise the Enter → split capture-action path.
            (100, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for SplitBlock {
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
            checks.push(check(
                self.position <= b.content_text().len(),
                Reason::PreconditionFailed,
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

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.push_undo_snapshot();
        let new_block_id = state.split_block(&self.block_id, self.position);
        // Production issues an editor_focus follow-up that moves keyboard focus
        // to the new block at position 0 (traits.rs::split_block → editor_focus_op).
        // The watch_editor_cursor reactor moves GPUI window.focus to the new
        // block's input, whose InputEvent::Focus calls `services.set_focus(new)`,
        // which is the engine-global `focused_block` mirror inv-focus-matches-ref
        // compares against. Mirror that here so subsequent transitions
        // (NavigateFocus, ArrowNavigate, JoinBlock, SplitBlock) and the
        // post-step invariants see the correct focused block.
        state
            .focused_entity_id
            .insert(Region::Main, new_block_id.clone());
        state
            .focused_cursor
            .insert(Region::Main, CursorPosition::start());
        state.focused_block = Some(new_block_id);
    }

    async fn apply_to_sut(&self, ref_state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_split_block(&self.block_id, self.position, ref_state)
            .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.active_watches.len();
        let blocks = state.block_state.blocks.len();
        let docs = state.documents.len();
        let update = expected_sql_for_kind(MutationKind::Update, watches, blocks, docs);
        let create = expected_sql_for_kind(MutationKind::Create, watches, blocks, docs);
        ExpectedSql {
            reads: update.reads + create.reads - REACTIVE_BASE,
            writes: update.writes + create.writes,
            ddl: 0,
            tolerance: update.tolerance + create.tolerance,
        }
    }
}
