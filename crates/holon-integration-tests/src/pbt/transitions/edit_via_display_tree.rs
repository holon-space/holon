//! Transition: edit a block's content via the display tree (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:899-913` (generator),
//! `state_machine.rs:3195-3207` (precondition),
//! `state_machine.rs:2485-2500` (ref-state apply),
//! `sut.rs:1942-2057` (SUT apply), and
//! `transition_budgets.rs:278-281` (expected SQL).

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

/// Edit a block's content via the display tree path.
#[derive(Clone, Debug)]
pub struct EditViaDisplayTree {
    pub block_id: EntityUri,
    pub new_content: String,
}

impl E2ETransitionFactory for EditViaDisplayTree {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (editable main-panel blocks) and let
        // `preconditions` be the single source of truth for which ones are
        // actually editable. Avoids duplicating content_type / layout /
        // focusable / atomic-editor / properly-setup checks across two sites.
        let candidates: Vec<EntityUri> = state
            .main_editable_descendants()
            .into_iter()
            .filter(|uri| {
                EditViaDisplayTree {
                    block_id: uri.clone(),
                    new_content: String::new(), // preconditions doesn't validate content
                }
                .preconditions(state)
                .is_good()
            })
            .collect();
        check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = (prop::sample::select(candidates), "[a-z ]{3,20}")
                .prop_map(|(block_id, new_content)| EditViaDisplayTree {
                    block_id,
                    new_content,
                })
                .boxed();
            (5, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for EditViaDisplayTree {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
            // Same gating rationale as `weighted_generator`: this transition
            // bypasses the keyboard pipeline. Atomic-editor primitives
            // cover the surface honestly. See `frontends/tui/TODO.md` A6.
            check(
                !ReferenceState::atomic_editor_enabled(),
                Reason::AtomicEditorActiveOverride,
            ),
        ];

        checks.push(check(
            state.block_state.blocks.contains_key(&self.block_id),
            Reason::FocusedBlockMissing,
        ));
        checks.push(check(
            state
                .block_state
                .blocks
                .get(&self.block_id)
                .is_some_and(|b| b.content_type == ContentType::Text),
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

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.push_undo_snapshot();
        state.apply_mutation(&crate::pbt::types::MutationEvent {
            source: crate::pbt::types::MutationSource::UI,
            mutation: crate::pbt::types::Mutation::Update {
                entity: "block".to_string(),
                id: self.block_id.clone(),
                fields: [(
                    "content".to_string(),
                    holon_api::Value::String(self.new_content.clone()),
                )]
                .into(),
            },
        });
        state.reset_cursor_if_focused(&self.block_id);
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_edit_via_display_tree(&self.block_id, &self.new_content)
            .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.active_watches.len();
        let blocks = state.block_state.blocks.len();
        let docs = state.documents.len();
        expected_sql_for_kind(MutationKind::Update, watches, blocks, docs)
    }
}
