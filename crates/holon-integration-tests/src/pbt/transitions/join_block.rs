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

use super::E2ETransitionImpl;
use crate::pbt::reference_state::{CursorPosition, ReferenceState};
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

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
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }

        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let no_content_update: std::collections::HashSet<EntityUri> = state
            .layout_blocks
            .render_source_ids
            .iter()
            .chain(state.layout_blocks.query_source_ids.iter())
            .chain(state.profile_block_ids.iter())
            .cloned()
            .collect();

        let peer_modified: std::collections::HashSet<String> = state
            .peers
            .iter()
            .flat_map(|p| p.modified_stable_ids.iter().cloned())
            .collect();
        let is_peer_modified = |id: &EntityUri| peer_modified.contains(id.id());

        // Editable blocks: non-page text blocks that are user-content (not layout),
        // not peer-modified, and are descendants of the main focus roots.
        let editable_block_ids: Vec<EntityUri> = state
            .block_state
            .blocks
            .iter()
            .filter(|(id, b)| {
                b.content_type == ContentType::Text
                    && !b.is_page()
                    && !state.layout_blocks.contains(id)
                    && !is_peer_modified(id)
                    && !no_content_update.contains(id)
                    && state.is_descendant_of_any(id, &focus_roots)
            })
            .map(|(id, _)| id.clone())
            .collect();

        // JoinBlock: two cases that both fire on Backspace at position 0.
        //   1. Block has a previous text sibling → merge into prev sibling.
        //   2. Block is the first child of a text parent → merge into parent.
        // Either case requires the merge target to be a text block (joining
        // into a headline / source / document has different semantics we
        // don't model). The parent target also must not be a layout
        // headline, since those host their own render expression and
        // mutating their content would corrupt the active layout.
        let joinable: Vec<EntityUri> = editable_block_ids
            .iter()
            .filter(|id| {
                // Case 1: prev sibling is text
                let prev_text = state.previous_sibling(id).is_some_and(|prev| {
                    state
                        .block_state
                        .blocks
                        .get(&prev)
                        .is_some_and(|b| b.content_type == ContentType::Text)
                });
                if prev_text {
                    return true;
                }
                // Case 2: no prev sibling (first child) and parent is
                // a non-layout text block.
                if state.previous_sibling(id).is_some() {
                    return false;
                }
                let parent_id = match state.block_state.blocks.get(*id) {
                    Some(b) => b.parent_id.clone(),
                    None => return false,
                };
                if parent_id.is_no_parent() || parent_id.is_sentinel() {
                    return false;
                }
                let parent_is_text = state
                    .block_state
                    .blocks
                    .get(&parent_id)
                    .is_some_and(|b| b.content_type == ContentType::Text);
                parent_is_text && !state.layout_blocks.contains(&parent_id)
            })
            .cloned()
            .collect();

        if joinable.is_empty() {
            return None;
        }

        let strat = proptest::sample::select(joinable)
            .prop_map(|block_id| JoinBlock { block_id })
            .boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for JoinBlock {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let focused_in_main = state.focused_entity(holon_api::Region::Main);
        let base_ok = state.app_started
            && state.is_properly_setup()
            && focused_in_main == Some(&self.block_id)
            && state
                .block_state
                .blocks
                .get(&self.block_id)
                .is_some_and(|b| b.content_type == ContentType::Text)
            && !state.layout_blocks.contains(&self.block_id)
            && state.is_descendant_of_any(&self.block_id, &focus_roots);
        if !base_ok {
            return false;
        }
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
        if prev_text {
            return true;
        }
        // Case 2: no previous sibling AND parent is a non-layout text block
        // → join into parent. Mirrors the production semantics added
        // for child→parent join.
        if state.previous_sibling(&self.block_id).is_some() {
            return false;
        }
        let parent_id = match state.block_state.blocks.get(&self.block_id) {
            Some(b) => b.parent_id.clone(),
            None => return false,
        };
        if parent_id.is_no_parent() || parent_id.is_sentinel() {
            return false;
        }
        let parent_is_text = state
            .block_state
            .blocks
            .get(&parent_id)
            .is_some_and(|b| b.content_type == ContentType::Text);
        parent_is_text && !state.layout_blocks.contains(&parent_id)
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
