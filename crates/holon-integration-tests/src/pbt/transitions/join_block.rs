//! Transition: join a block into its previous sibling (or parent).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1194-1245` (generator),
//! `state_machine.rs:3438-3486` (precondition),
//! `state_machine.rs:2693-2718` (ref-state apply),
//! `sut.rs:4129-4147` (SUT apply), and
//! `transition_budgets.rs:314-323` (expected SQL).

use holon_api::entity_uri::EntityUri;
use holon_api::{ContentType, Region};
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

/// Join a block into its previous text sibling, or (when first child) into
/// its non-layout text parent. Mirrors Backspace-at-position-0 semantics.
#[derive(Clone, Debug)]
pub struct JoinBlock {
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for JoinBlock {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // JoinBlock fires on the focused Main entity (preconditions
        // gate `focused_in_main == self.block_id`). Single-candidate
        // pattern mirrors move_up.rs / move_down.rs.
        let Some(block_id) = state.focused_entity(Region::Main).cloned() else {
            return Validated::fail(Reason::NoFocusInMain);
        };
        let instance = JoinBlock { block_id };
        instance.preconditions(state).map(|()| {
            let strat = Just(instance.clone()).boxed();
            (1, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for JoinBlock {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let focused_in_main = state.focused_entity(holon_api::Region::Main);
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
        ];

        checks.push(check(
            focused_in_main == Some(&self.block_id),
            Reason::FocusedIsNotSelf,
        ));

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

        // Case 1: previous text sibling exists → join into prev sibling.
        let prev_text = state
            .previous_sibling(&self.block_id)
            .and_then(|prev| {
                state
                    .block_state
                    .blocks
                    .get(&prev)
                    .map(|b| b.content_type == ContentType::Text)
            })
            .unwrap_or(false);

        // Case 2: no previous sibling AND parent is a non-layout text block.
        let parent_ok = if !prev_text {
            if state.previous_sibling(&self.block_id).is_some() {
                false
            } else {
                let parent_id = match state.block_state.blocks.get(&self.block_id) {
                    Some(b) => b.parent_id.clone(),
                    None => {
                        return checks
                            .into_iter()
                            .collect::<Validated<Vec<()>, _>>()
                            .map(|_| ());
                    }
                };
                if parent_id.is_no_parent() || parent_id.is_sentinel() {
                    false
                } else {
                    state
                        .block_state
                        .blocks
                        .get(&parent_id)
                        .is_some_and(|b| b.content_type == ContentType::Text)
                        && !state.layout_blocks.contains(&parent_id)
                }
            }
        } else {
            true
        };

        checks.push(check(prev_text || parent_ok, Reason::PreconditionFailed));

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.push_undo_snapshot();
        // Determine the merge target before mutation: prev sibling if
        // present, otherwise the parent block (child→parent join).
        let target_id = state.previous_sibling(&self.block_id).unwrap_or_else(|| {
            state
                .block_state
                .blocks
                .get(&self.block_id)
                .map(|b| b.parent_id.clone())
                .expect("JoinBlock precondition: block must exist with a parent")
        });
        state.join_block(&self.block_id);
        // Focus moves to the merge target (prev sibling OR parent);
        // cursor lands at the join boundary, but the reference model
        // tracks (line, column) — match SplitBlock's behaviour and
        // reset to start. Production sets cursor at join boundary
        // via the editor_focus follow-up; PBT cursor checks are
        // best-effort and do not gate the test.
        state.focused_entity_id.insert(Region::Main, target_id);
        state
            .focused_cursor
            .insert(Region::Main, CursorPosition::start());
    }

    async fn apply_to_sut(&self, ref_state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_join_block(&self.block_id, ref_state).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.active_watches.len();
        let blocks = state.block_state.blocks.len();
        let docs = state.documents.len();
        let update = expected_sql_for_kind(MutationKind::Update, watches, blocks, docs);
        let delete = expected_sql_for_kind(MutationKind::Delete, watches, blocks, docs);
        ExpectedSql {
            reads: update.reads + delete.reads - REACTIVE_BASE,
            writes: update.writes + delete.writes,
            ddl: 0,
            tolerance: update.tolerance + delete.tolerance,
        }
    }
}
