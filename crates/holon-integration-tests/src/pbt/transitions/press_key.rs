//! Transition: press a structural key chord in the active editor.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1682-1717` (generator),
//! `state_machine.rs:3558-3560` (precondition),
//! `state_machine.rs:2975-3051` (ref-state apply),
//! `sut.rs:4430-4463` (SUT apply), and
//! `transition_budgets.rs:378-387` (expected SQL).

use crate::pbt::validation::{Reason, check};
use holon_api::KeyChord;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::SutHandle;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, MutationKind, REACTIVE_BASE, expected_sql_for_kind,
};

/// Press a structural key chord (Enter, Backspace, Escape) in the active editor.
/// Gated to `PBT_ATOMIC_EDITOR=1` runs.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PressKey {
    pub chord: KeyChord,
}

impl TransitionFactory<ReferenceState> for PressKey {
    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // ADR 0009 asymmetry #1: "edit content" is exercisable on **any** block
        // store, not just Loro — under Turso-only the on-blur `set_field` path
        // persists editor content. Gating to `AnyStorageOf({Loro, Turso})`
        // makes the editor path bisectable across the storage axis. Headless
        // Turso-only slices are unaffected: `preconditions` still requires
        // `enable_loro() || real_editor_enabled()`, so without a real editor the
        // transition is rejected dynamically just as the old structural gate did.
        ::holon_pbt_core::RequiredWiring::any_storage_of([
            ::holon_pbt_core::StorageAdapter::Loro,
            ::holon_pbt_core::StorageAdapter::Turso,
        ])
    }

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Verify preconditions hold. All structural gates are delegated to preconditions().
        let sample_chord = holon_api::KeyChord(std::iter::once(holon_api::Key::Enter).collect());
        let app_check = PressKey {
            chord: sample_chord.clone(),
        }
        .preconditions(state);
        match app_check {
            Validated::Fail(reasons) => return Validated::Fail(reasons),
            Validated::Good(_) => {}
        }

        let last = state.action.last_transition_kind;
        let pending_edit = state
            .ui
            .tab
            .active_editor
            .as_ref()
            .map(|e| {
                state
                    .domain
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
        ];

        let strat = chord_strategy.prop_map(|chord| PressKey { chord }).boxed();
        Validated::Good((pk_weight, strat))
    }
}

impl TransitionRef<ReferenceState> for PressKey {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(
                ReferenceState::atomic_editor_enabled(),
                Reason::AtomicEditorDisabled,
            ),
            check(
                state.enable_loro() || ReferenceState::real_editor_enabled(),
                Reason::LoroRequiredForAtomicEditor,
            ),
            check(state.action.app_started, Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
            check(
                state.current_focus(holon_api::Region::Main).is_some(),
                Reason::NoFocusInMain,
            ),
            check(state.ui.tab.active_editor.is_some(), Reason::NoActiveEditor),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        use holon_api::Key;
        use holon_pbt_core::capabilities::RefFocusMut;

        // Preconditions guarantee an active editor; replay/minimization can
        // apply transitions outside generation-time preconditions, and a
        // silent no-op there desyncs ref vs SUT invisibly.
        let editor = state
            .ui
            .tab
            .active_editor
            .clone()
            .expect("PressKey::apply_to_ref: no active editor (preconditions violated)");
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

        // Enter (no modifier): commit pending edit, then split at cursor
        // against the post-commit content. Split semantics (incl. the ADR-0010
        // focus + active-editor handoff to the new block) are owned by the
        // shared cap fn — one implementation for SplitBlock AND PressKey, so
        // the two can't drift (the drift hosted the SplitBlock Heisenbug).
        if matches!(single, Some(Key::Enter)) && !has_modifier {
            state.commit_active_editor_if_changed();
            crate::pbt::transitions::split_block::split_block_apply_to_ref(
                &block_id,
                cursor_byte,
                state,
            );
        }
        // Backspace at position 0: commit, then join — IF a merge target
        // exists. Unlike `JoinBlock` (whose preconditions guarantee a target),
        // a Backspace-at-0 chord can land on a first child with no joinable
        // parent; prod's `join_block` op finds no merge target and no-ops, so
        // the ref no-ops too. When a target exists, join semantics are the
        // shared cap fn's.
        else if matches!(single, Some(Key::Backspace)) && !has_modifier && cursor_byte == 0 {
            state.commit_active_editor_if_changed();
            let prev = state.previous_sibling(&block_id);
            let parent = state
                .domain
                .block_state
                .blocks
                .get(&block_id)
                .map(|b| b.parent_id.clone());
            let joinable = match (&prev, &parent) {
                (Some(_), _) => true,
                (None, Some(p)) => {
                    // Only join into parent if parent is a non-layout text block.
                    state
                        .domain
                        .block_state
                        .blocks
                        .get(p)
                        .is_some_and(|b| b.content_type == holon_api::ContentType::Text)
                        && !state.domain.layout_blocks.contains(p)
                        && !p.is_no_parent()
                        && !p.is_sentinel()
                }
                _ => false,
            };
            if joinable {
                crate::pbt::transitions::join_block::join_block_apply_to_ref(&block_id, state);
                // The joined (deleted) block's editor closes; prod's follow-up
                // re-focus lands on the merge target via the op response, which
                // `join_block_apply_to_ref`'s `set_focus` already models.
                state.close_active_editor();
            }
        }
        // Backspace at cursor > 0: production's `InputState` removes one
        // character before the cursor. No structural change. Mirror that
        // on `active_editor.in_memory_content` so `inv-displayed-text`'s
        // expected (= in_memory_content while editor is active) tracks
        // what's actually on screen.
        else if matches!(single, Some(Key::Backspace))
            && !has_modifier
            && cursor_byte > 0
            && let Some(editor) = state.ui.tab.active_editor.as_mut()
        {
            editor.delete_backward(1);
        }
        // Other chords (Tab, etc.): no structural change modeled in v1.
        // Pending edits remain in InputState.
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for PressKey {
    async fn apply_to_sut(&self, ref_state: &ReferenceState, sut: &mut S) {
        sut.apply_press_key(&self.chord, ref_state).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for PressKey {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.mcp.active_watches.len();
        let blocks = state.domain.block_state.blocks.len();
        let docs = state.files.documents.len();
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
