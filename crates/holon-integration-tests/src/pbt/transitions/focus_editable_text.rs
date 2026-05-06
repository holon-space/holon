//! Transition: focus an editable text block in the main region.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1563-1628` (generator),
//! `state_machine.rs:3529-3550` (precondition),
//! `state_machine.rs:2935-2955` (ref-state apply),
//! `sut.rs:4343-4394` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use crate::pbt::validation::{Reason, check};
use holon_api::ContentType;
use holon_api::entity_uri::EntityUri;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::{ActiveEditor, ReferenceState};
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE};

/// Focus a live-rendered editable text block in the main panel.
/// Gated to `PBT_ATOMIC_EDITOR=1` runs.
#[derive(Clone, Debug)]
pub struct FocusEditableText {
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for FocusEditableText {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (text blocks in main panel) and let
        // `preconditions` be the single source of truth for which ones are
        // actually focusable. Avoids duplicating the content_type / layout /
        // focusable / page checks across two sites.
        let candidates: Vec<EntityUri> = state
            .block_state
            .blocks
            .iter()
            .map(|(id, _)| id.clone())
            .filter(|uri| {
                FocusEditableText {
                    block_id: uri.clone(),
                }
                .preconditions(state)
                .is_good()
            })
            .collect();

        check(!candidates.is_empty(), Reason::NoFocusableBlocks).map(|_| {
            let last = state.last_transition_kind;
            let weight = match last {
                Some("StartApp")
                | Some("NavigateFocus")
                | Some("NavigateSidebar")
                | Some("ClickBlock") => 5,
                _ => 2,
            };

            let strat = proptest::sample::select(candidates)
                .prop_map(|block_id| FocusEditableText { block_id })
                .boxed();
            (weight, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for FocusEditableText {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let no_content_update: std::collections::HashSet<EntityUri> = state
            .layout_blocks
            .render_source_ids
            .iter()
            .chain(state.layout_blocks.query_source_ids.iter())
            .chain(state.profile_block_ids.iter())
            .cloned()
            .collect();

        let checks: Vec<Validated<(), Reason>> = vec![
            check(
                ReferenceState::atomic_editor_enabled(),
                Reason::AtomicEditorDisabled,
            ),
            // See `press_key.rs` — atomic editor primitives need a Loro
            // path to carry per-keystroke writes; SqlOnly has none.
            check(
                state.variant.enable_loro,
                Reason::LoroRequiredForAtomicEditor,
            ),
            check(state.app_started, Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
            check(
                state.current_focus(holon_api::Region::Main).is_some(),
                Reason::NoFocusInMain,
            ),
            // Only when no editor is active (FocusEditableText opens an editor;
            // continue using it with MoveCursor/TypeChars/... while active).
            check(state.active_editor.is_none(), Reason::NoActiveEditor),
            check(
                state
                    .block_state
                    .blocks
                    .get(&self.block_id)
                    .is_some_and(|b| b.content_type == ContentType::Text && !b.is_page()),
                Reason::FocusedNotText,
            ),
            check(
                !state.layout_blocks.contains(&self.block_id),
                Reason::FocusedInLayoutBlocks,
            ),
            check(
                state.is_descendant_of_any(&self.block_id, &focus_roots),
                Reason::FocusedNotDescendantOfFocusRoot,
            ),
            check(
                !no_content_update.contains(&self.block_id),
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let saved = state
            .block_state
            .blocks
            .get(&self.block_id)
            .map(|b| b.content.clone())
            .unwrap_or_default();
        let cursor_byte = saved.len();
        state.active_editor = Some(ActiveEditor {
            block_id: self.block_id.clone(),
            in_memory_content: saved,
            cursor_byte,
        });
        // Deliberately do not update navigation focus here. While an
        // editor is active, `active_editor.block_id` is the source of
        // truth for editor focus and the engine's global `focused_block`
        // is a transient implementation detail (the click handler may or
        // may not propagate through to the ui_state mirror depending on
        // whether the GPUI window has fully painted). inv-focus-matches-ref is gated on
        // `active_editor.is_none()` to skip exactly this window.
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_focus_editable_text(&self.block_id).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}
