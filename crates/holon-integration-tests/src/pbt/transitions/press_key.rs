//! Transition: press a structural key chord in the active editor.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1682-1717` (generator),
//! `state_machine.rs:3558-3560` (precondition),
//! `state_machine.rs:2975-3051` (ref-state apply),
//! `sut.rs:4430-4463` (SUT apply), and
//! `transition_budgets.rs:378-387` (expected SQL).

use holon_api::KeyChord;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, MutationKind, REACTIVE_BASE, expected_sql_for_kind,
};

/// Press a structural key chord (Enter, Backspace, Escape) in the active editor.
/// Gated to `PBT_ATOMIC_EDITOR=1` runs.
#[derive(Clone, Debug)]
pub struct PressKey {
    pub chord: KeyChord,
}

impl E2ETransitionFactory for PressKey {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !ReferenceState::atomic_editor_enabled() {
            return None;
        }
        if !state.app_started {
            return None;
        }
        if !state.is_properly_setup() {
            return None;
        }
        if state.current_focus(holon_api::Region::Main).is_none() {
            return None;
        }
        if state.active_editor.is_none() {
            return None;
        }

        let last = state.last_transition_kind;
        let pending_edit = state
            .active_editor
            .as_ref()
            .map(|e| {
                state
                    .block_state
                    .blocks
                    .get(&e.block_id)
                    .is_some_and(|b| b.content != e.in_memory_content)
            })
            .unwrap_or(false);

        let pk_weight = if pending_edit {
            10 // pending in-memory edit + chord = the bug class
        } else {
            match last {
                Some("TypeChars") | Some("DeleteBackward") => 6,
                Some("MoveCursor") => 3,
                _ => 1,
            }
        };

        let chord_strategy = prop_oneof![
            // Enter (no modifier) → split_block path.
            3 => Just(holon_api::KeyChord(
                std::iter::once(holon_api::Key::Enter).collect()
            )),
            // Backspace (no modifier) — only structural at cursor=0,
            // but the SUT issues it unconditionally and the system
            // routes mid-line backspace to InputState. Both paths
            // are useful coverage.
            2 => Just(holon_api::KeyChord(
                std::iter::once(holon_api::Key::Backspace).collect()
            )),
            // Escape — blur-ish; production may discard pending.
            1 => Just(holon_api::KeyChord(
                std::iter::once(holon_api::Key::Escape).collect()
            )),
        ];

        let strat = chord_strategy.prop_map(|chord| PressKey { chord }).boxed();
        Some((pk_weight, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for PressKey {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        ReferenceState::atomic_editor_enabled() && state.active_editor.is_some()
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        use holon_api::{Key, Region};

        let Some(editor) = state.active_editor.clone() else {
            return;
        };
        let block_id = editor.block_id.clone();
        let cursor_byte = editor.cursor_byte;

        let has_modifier = self
            .chord
            .0
            .iter()
            .any(|k| matches!(k, Key::Cmd | Key::Ctrl | Key::Alt | Key::Shift));
        let regulars: Vec<Key> = self
            .chord
            .0
            .iter()
            .filter(|k| !matches!(k, Key::Cmd | Key::Ctrl | Key::Alt | Key::Shift))
            .cloned()
            .collect();
        let single = if regulars.len() == 1 {
            Some(regulars[0].clone())
        } else {
            None
        };

        // Enter (no modifier): commit pending edit, then split
        // at cursor against the post-commit content.
        if matches!(single, Some(Key::Enter)) && !has_modifier {
            state.commit_active_editor_if_changed();
            state.push_undo_snapshot();
            state.split_block(&block_id, cursor_byte);
            state.reset_cursor_if_focused(&block_id);
            state.active_editor = None;
            state.focused_entity_id.remove(&Region::Main);
        }
        // Backspace at position 0: commit, then join.
        else if matches!(single, Some(Key::Backspace)) && !has_modifier && cursor_byte == 0 {
            state.commit_active_editor_if_changed();
            let prev = state.previous_sibling(&block_id);
            let parent = state
                .block_state
                .blocks
                .get(&block_id)
                .map(|b| b.parent_id.clone());
            let target_id = match (&prev, &parent) {
                (Some(p), _) => Some(p.clone()),
                (None, Some(p)) => {
                    // Only join into parent if parent is a non-layout text block.
                    let parent_ok = state
                        .block_state
                        .blocks
                        .get(p)
                        .is_some_and(|b| b.content_type == holon_api::ContentType::Text)
                        && !state.layout_blocks.contains(p)
                        && !p.is_no_parent()
                        && !p.is_sentinel();
                    if parent_ok { Some(p.clone()) } else { None }
                }
                _ => None,
            };
            if let Some(target_id) = target_id {
                state.push_undo_snapshot();
                state.join_block(&block_id);
                state.focused_entity_id.insert(Region::Main, target_id);
                state.focused_cursor.insert(
                    Region::Main,
                    crate::pbt::reference_state::CursorPosition::start(),
                );
                state.active_editor = None;
            }
        }
        // Other chords (Tab, Escape, etc.): no structural change
        // modeled in v1. Pending edits remain in InputState.
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_press_key(&self.chord).await;
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
