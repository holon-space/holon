//! Transition: focus an editable text block in the main region.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1563-1628` (generator),
//! `state_machine.rs:3529-3550` (precondition),
//! `state_machine.rs:2935-2955` (ref-state apply),
//! `sut.rs:4343-4394` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use holon_api::ContentType;
use holon_api::entity_uri::EntityUri;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

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
        // Only when no editor is active (FocusEditableText opens an editor;
        // continue using it with MoveCursor/TypeChars/... while active).
        if state.active_editor.is_some() {
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

        let main_focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let live_rendered = super::super::live_geometry::rendered_entity_ids();
        let focus_candidates: Vec<EntityUri> = state
            .block_state
            .blocks
            .iter()
            .filter(|(id, b)| {
                b.content_type == ContentType::Text
                    && !b.is_page()
                    && !state.layout_blocks.contains(id)
                    && state.is_descendant_of_any(id, &main_focus_roots)
                    && !no_content_update.contains(id)
                    && live_rendered
                        .as_ref()
                        .is_some_and(|s| s.contains(id.as_str()))
            })
            .map(|(id, _)| id.clone())
            .collect();

        if focus_candidates.is_empty() {
            return None;
        }

        let last = state.last_transition_kind;
        let weight = match last {
            Some("StartApp")
            | Some("NavigateFocus")
            | Some("NavigateSidebar")
            | Some("ClickBlock")
            | Some("Blur") => 5,
            _ => 2,
        };

        let strat = proptest::sample::select(focus_candidates)
            .prop_map(|block_id| FocusEditableText { block_id })
            .boxed();
        Some((weight, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for FocusEditableText {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        if !ReferenceState::atomic_editor_enabled() {
            return false;
        }
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        state.app_started
            && state.is_properly_setup()
            && state.current_focus(holon_api::Region::Main).is_some()
            && state
                .block_state
                .blocks
                .get(&self.block_id)
                .is_some_and(|b| b.content_type == ContentType::Text && !b.is_page())
            && !state.layout_blocks.contains(&self.block_id)
            && state.is_descendant_of_any(&self.block_id, &focus_roots)
            && super::super::live_geometry::is_entity_rendered(self.block_id.as_str())
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
        // whether the GPUI window has fully painted). inv15 is gated on
        // `active_editor.is_none()` to skip exactly this window.
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_focus_editable_text(&self.block_id).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}
