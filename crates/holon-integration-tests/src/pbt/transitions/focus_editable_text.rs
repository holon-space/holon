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
use holon_pbt_core::capabilities::{SutDriver, SutLayout};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use std::time::Duration;
use validated::Validated;

use crate::pbt::reference_state::{ActiveEditor, ReferenceState};
use crate::pbt::transition_dispatch::SutHandle;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE};

// ── Capability-bound free function (Phase C) ──────────────────────

/// SUT-side body of `FocusEditableText`. Bound on `SutLayout + SutDriver`
/// so any slice supplying both capabilities can include this transition.
///
/// Fails loud per CLAUDE.md: bounds must resolve within 5 s and the click
/// must succeed. The 5-second budget mirrors GPUI's scroll → layout →
/// flush window for offscreen virtualized list items
/// (`wait_for_entity_bounds` polls for ~200 ms before issuing a
/// scroll-into-view RPC, then keeps polling).
pub async fn apply_focus_editable_text_to_sut<S: SutLayout + SutDriver>(
    sut: &mut S,
    id: &EntityUri,
) {
    sut.wait_for_bounds(id, Duration::from_secs(5))
        .await
        .unwrap_or_else(|e| panic!("[FocusEditableText] bounds unavailable for {id}: {e}"));
    sut.click_entity(id, "main")
        .await
        .unwrap_or_else(|e| panic!("[FocusEditableText] click_entity failed for {id}: {e}"));
}

/// Focus a live-rendered editable text block in the main panel.
/// Gated to `PBT_ATOMIC_EDITOR=1` runs.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FocusEditableText {
    pub block_id: EntityUri,
}

impl TransitionFactory<ReferenceState> for FocusEditableText {
    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (text blocks in main panel) and let
        // `preconditions` be the single source of truth for which ones are
        // actually focusable. Avoids duplicating the content_type / layout /
        // focusable / page checks across two sites.
        let candidates: Vec<EntityUri> = state
            .block_state
            .blocks
            .keys()
            .filter(|&uri| {
                FocusEditableText {
                    block_id: uri.clone(),
                }
                .preconditions(state)
                .is_good()
            })
            .cloned()
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

impl TransitionRef<ReferenceState> for FocusEditableText {
    type Reason = Reason;

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
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for FocusEditableText {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.apply_focus_editable_text(&self.block_id).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for FocusEditableText {
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}
