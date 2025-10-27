//! Transition: indent the focused block (make it a child of its previous sibling).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1106-1121` (generator),
//! `state_machine.rs:3314-3328` (precondition),
//! `state_machine.rs:2648-2660` (ref-state apply),
//! `sut.rs:3541-3546` (SUT apply), and
//! `transition_budgets.rs:298-302` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

use holon_api::{ContentType, EntityUri};

/// Indent the focused block: re-parent it under its previous sibling via the
/// `Alt+Right` / Tab chord.
#[derive(Clone, Debug)]
pub struct Indent {
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for Indent {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let focused_in_main = state.focused_entity(holon_api::Region::Main).cloned();
        let no_content_update: std::collections::HashSet<EntityUri> = state
            .layout_blocks
            .render_source_ids
            .iter()
            .chain(state.layout_blocks.query_source_ids.iter())
            .chain(state.profile_block_ids.iter())
            .cloned()
            .collect();
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
        let indentable: Vec<EntityUri> = editable_block_ids
            .iter()
            .filter(|id| state.previous_sibling(id).is_some())
            .cloned()
            .collect();
        if indentable.is_empty() {
            return None;
        }
        let strat = proptest::sample::select(indentable)
            .prop_map(|block_id| Indent { block_id })
            .boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for Indent {
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
                .is_some_and(|b| b.content_type == ContentType::Text)
            && !state.layout_blocks.contains(&self.block_id)
            && state.is_descendant_of_any(&self.block_id, &focus_roots)
            && state.previous_sibling(&self.block_id).is_some()
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

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
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
