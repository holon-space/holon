//! Transition: trigger slash command (delete) on the currently-focused block.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1057-1081` (generator),
//! `state_machine.rs:3263-3277` (precondition),
//! `state_machine.rs:2535-2545` (ref-state apply),
//! `sut.rs:3250-3362` (SUT apply), and
//! `transition_budgets.rs:284-286` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

use holon_api::{ContentType, EntityUri};

/// Trigger the "/" slash-command menu on the focused block and select "delete".
#[derive(Clone, Debug)]
pub struct TriggerSlashCommand {
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for TriggerSlashCommand {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let focused_in_main = state.focused_entity(holon_api::Region::Main).cloned();
        let deletable_block_ids: Vec<EntityUri> =
            if state.is_properly_setup() && focused_in_main.is_some() {
                let focused = focused_in_main.as_ref().unwrap();
                let valid = state.block_state.blocks.get(focused).is_some_and(|b| {
                    b.content_type == ContentType::Text
                        && !state.layout_blocks.contains(&b.id)
                        && !b.id.as_str().contains("default-")
                        && state.block_state.blocks.len() > 2
                        && state.is_descendant_of_any(&b.id, &focus_roots)
                });
                if valid { vec![focused.clone()] } else { vec![] }
            } else {
                vec![]
            };
        if deletable_block_ids.is_empty() {
            return None;
        }
        let strat = proptest::sample::select(deletable_block_ids)
            .prop_map(|block_id| TriggerSlashCommand { block_id })
            .boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for TriggerSlashCommand {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        state.app_started
            && state.is_properly_setup()
            && state.block_state.blocks.contains_key(&self.block_id)
            && state
                .block_state
                .blocks
                .get(&self.block_id)
                .is_some_and(|b| b.content_type == ContentType::Text)
            && !state.layout_blocks.contains(&self.block_id)
            && !self.block_id.as_str().contains("default-")
            && state.block_state.blocks.len() > 2
            && state.is_descendant_of_any(&self.block_id, &focus_roots)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        use crate::pbt::types::{Mutation, MutationEvent, MutationSource};
        state.push_undo_snapshot();
        state.apply_mutation(&MutationEvent {
            source: MutationSource::UI,
            mutation: Mutation::Delete {
                entity: "block".to_string(),
                id: self.block_id.clone(),
            },
        });
        state.clear_focus_if_deleted(&self.block_id);
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_trigger_slash_command(&self.block_id).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        expected_sql_for_kind(
            MutationKind::Delete,
            state.active_watches.len(),
            state.block_state.blocks.len(),
            state.documents.len(),
        )
    }
}
