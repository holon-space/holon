//! Transition: drag the focused block onto a target, making it a child of the target.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1248-1319` (generator),
//! `state_machine.rs:3374-3425` (precondition),
//! `state_machine.rs:2679-2685` (ref-state apply),
//! `sut.rs:3569-3598` (SUT apply), and
//! `transition_budgets.rs:298-302` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

use crate::pbt::state_machine::DRAG_DROP_ENABLED;
use holon_api::{ContentType, EntityUri};

/// Drag the currently-focused block onto a target block, re-parenting the source
/// as a child of the target at the beginning (after=None).
#[derive(Clone, Debug)]
pub struct DragDropBlock {
    pub source: EntityUri,
    pub target: EntityUri,
}

impl E2ETransitionFactory for DragDropBlock {
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
        let text_block_ids: Vec<EntityUri> = state
            .block_state
            .blocks
            .values()
            .filter(|b| b.content_type == ContentType::Text && !b.is_page())
            .map(|b| b.id.clone())
            .collect();

        let drag_source: Option<EntityUri> = if !editable_block_ids.is_empty() {
            Some(editable_block_ids[0].clone())
        } else {
            None
        };
        let drag_targets: Vec<EntityUri> = drag_source
            .as_ref()
            .map(|source| {
                text_block_ids
                    .iter()
                    .filter(|id| {
                        id != &source
                            && !state.layout_blocks.contains(id)
                            && state.is_descendant_of_any(id, &focus_roots)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if !DRAG_DROP_ENABLED || drag_source.is_none() || drag_targets.is_empty() {
            return None;
        }
        let block_state = state.block_state.clone();
        let source = drag_source.unwrap();
        let valid_targets: Vec<EntityUri> = drag_targets
            .into_iter()
            .filter(|t| {
                // Reject cycle: target descendant of source.
                let mut current = t.clone();
                for _ in 0..50 {
                    let Some(b) = block_state.blocks.get(&current) else {
                        return true;
                    };
                    if b.parent_id == source {
                        return false;
                    }
                    if b.parent_id.is_no_parent() || b.parent_id.is_sentinel() {
                        return true;
                    }
                    current = b.parent_id.clone();
                }
                true
            })
            // Reject no-op: target already source's parent.
            .filter(|t| {
                block_state
                    .blocks
                    .get(&source)
                    .map(|b| &b.parent_id != t)
                    .unwrap_or(false)
            })
            .collect();
        if valid_targets.is_empty() {
            return None;
        }
        let source_clone = source.clone();
        let strat = proptest::sample::select(valid_targets)
            .prop_map(move |target| DragDropBlock {
                source: source_clone.clone(),
                target,
            })
            .boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for DragDropBlock {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        if !state.app_started || !state.is_properly_setup() {
            return false;
        }
        if self.source == self.target {
            return false;
        }
        let focused_in_main = state.focused_entity(holon_api::Region::Main);
        if focused_in_main != Some(&self.source) {
            return false;
        }
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let is_text = |id: &EntityUri| {
            state
                .block_state
                .blocks
                .get(id)
                .is_some_and(|b| b.content_type == ContentType::Text)
        };
        if !is_text(&self.source) || !is_text(&self.target) {
            return false;
        }
        if state.layout_blocks.contains(&self.source) || state.layout_blocks.contains(&self.target)
        {
            return false;
        }
        if !state.is_descendant_of_any(&self.source, &focus_roots)
            || !state.is_descendant_of_any(&self.target, &focus_roots)
        {
            return false;
        }
        // No-op: target is already source's parent.
        if state
            .block_state
            .blocks
            .get(&self.source)
            .is_none_or(|b| &b.parent_id == &self.target)
        {
            return false;
        }
        // Cycle: target is a descendant of source.
        let mut singleton = std::collections::BTreeSet::new();
        singleton.insert(self.source.clone());
        if state.is_descendant_of_any(&self.target, &singleton) {
            return false;
        }
        true
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.push_undo_snapshot();
        // Production's drop_zone dispatches `move_block(id=source,
        // parent_id=target, after_block_id=None)` which inserts at
        // the beginning of the target's children.
        state.move_block(&self.source, self.target.clone(), None);
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_drag_drop_block(&self.source, &self.target).await;
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
