//! Transition: split a block at a byte position.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1171-1191` (generator),
//! `state_machine.rs:3426-3436` (precondition),
//! `state_machine.rs:2687-2691` (ref-state apply),
//! `sut.rs:4052-4127` (SUT apply), and
//! `transition_budgets.rs:303-312` (expected SQL).

use holon_api::ContentType;
use holon_api::entity_uri::EntityUri;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

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
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }

        let no_content_update: std::collections::HashSet<EntityUri> = state
            .layout_blocks
            .render_source_ids
            .iter()
            .chain(state.layout_blocks.query_source_ids.iter())
            .chain(state.profile_block_ids.iter())
            .cloned()
            .collect();

        // SplitBlock shares the `editable_block_ids` context from the legacy
        // post-startup generator: only the focused block (if valid) is
        // editable. Content is ASCII-only in PBT, so byte == char.
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let focused_in_main = state.focused_entity(holon_api::Region::Main).cloned();
        let editable_block_ids: Vec<EntityUri> =
            if state.is_properly_setup() && focused_in_main.is_some() {
                let focused = focused_in_main.as_ref().unwrap();
                let valid = state
                    .block_state
                    .blocks
                    .get(focused)
                    .is_some_and(|b| b.content_type == ContentType::Text && !b.is_page())
                    && state.layout_blocks.is_focusable(focused)
                    && !no_content_update.contains(focused)
                    && state.is_descendant_of_any(focused, &focus_roots);
                if valid { vec![focused.clone()] } else { vec![] }
            } else {
                vec![]
            };

        if editable_block_ids.is_empty() {
            return None;
        }

        let blocks = state.block_state.blocks.clone();
        let strat = proptest::sample::select(editable_block_ids)
            .prop_flat_map(move |block_id| {
                let content_len = blocks
                    .get(&block_id)
                    .map(|b| b.content_text().len())
                    .unwrap_or(0);
                (Just(block_id), 0..=content_len)
            })
            .prop_map(|(block_id, position)| SplitBlock { block_id, position })
            .boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for SplitBlock {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let focused_in_main = state.focused_entity(holon_api::Region::Main);
        state.app_started
            && state.is_properly_setup()
            && focused_in_main == Some(&self.block_id)
            && state
                .block_state
                .blocks
                .get(&self.block_id)
                .is_some_and(|b| {
                    b.content_type == ContentType::Text && self.position <= b.content_text().len()
                })
            && !state.layout_blocks.contains(&self.block_id)
            && state.is_descendant_of_any(&self.block_id, &focus_roots)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.push_undo_snapshot();
        state.split_block(&self.block_id, self.position);
        state.reset_cursor_if_focused(&self.block_id);
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
