//! Transition: focus an editable text block in the main region.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1563-1628` (generator),
//! `state_machine.rs:3529-3550` (precondition),
//! `state_machine.rs:2935-2955` (ref-state apply),
//! `sut.rs:4343-4394` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use crate::pbt::validation::{Reason, check};
use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::{
    CapRegion, RefBlockTree, RefBlockTreeMut, RefEditorMirror, RefEditorMirrorMut, RefFocus,
    RefFocusMut, RefFocusRoots, RefLayout, RefLifecycle, SutDriver, SutLayout,
    commit_active_editor_if_dirty,
};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use std::time::Duration;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
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
pub async fn apply_focus_editable_text_to_sut<S: SutLayout + SutDriver>(sut: &S, id: &EntityUri) {
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
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutFocusWrite,
        >()]
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // ADR 0009 asymmetry #1 (same as TypeChars/PressKey): editor primitives
        // work on any block store, so gate to AnyStorageOf({Loro, Turso}) —
        // without this, `active_editor` is never set under Turso-only and the
        // widened TypeChars/PressKey gates are unreachable in exactly the
        // configuration they were widened for.
        ::holon_pbt_core::RequiredWiring::any_storage_of([
            ::holon_pbt_core::StorageAdapter::Loro,
            ::holon_pbt_core::StorageAdapter::Turso,
        ])
    }

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (text blocks in main panel) and let
        // `preconditions` be the single source of truth for which ones are
        // actually focusable. Avoids duplicating the content_type / layout /
        // focusable / page checks across two sites.
        let candidates: Vec<EntityUri> = state
            .domain
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
            let last = state.action.last_transition_kind;
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

pub fn focus_editable_text_preconditions<
    R: RefLifecycle + RefFocus + RefFocusRoots + RefEditorMirror + RefBlockTree + RefLayout,
>(
    block_id: &EntityUri,
    state: &R,
) -> Validated<(), Reason> {
    let focus_roots = state.expected_focus_root_ids(CapRegion::Main);

    let checks: Vec<Validated<(), Reason>> = vec![
        check(state.has_editor_buffer(), Reason::NoEditorBuffer),
        check(state.app_started(), Reason::AppNotStarted),
        check(state.is_properly_setup(), Reason::NotProperlySetup),
        // Block-interaction transitions need the block to render as an
        // interactive widget (ops/draggable) reactively over the navigated
        // focus. Only the default layout does; custom `index.org` query
        // layouts don't (see RefLifecycle::renders_block_interactively).
        check(
            RefLifecycle::renders_block_interactively(state, block_id),
            Reason::BlocksNotInteractiveUnderLayout,
        ),
        check(
            state.current_focus(CapRegion::Main).is_some(),
            Reason::NoFocusInMain,
        ),
        // Only when no editor is active (FocusEditableText opens an editor;
        // continue using it with MoveCursor/TypeChars/... while active).
        check(
            state.active_editor_block().is_none(),
            Reason::NoActiveEditor,
        ),
        check(
            state.is_text_block(block_id) && !state.is_page_block(block_id),
            Reason::FocusedNotText,
        ),
        check(
            !state.is_layout_block(block_id),
            Reason::FocusedInLayoutBlocks,
        ),
        check(
            state.is_descendant_of_any(block_id, &focus_roots),
            Reason::FocusedNotDescendantOfFocusRoot,
        ),
        check(
            !state.is_no_content_update(block_id),
            Reason::PreconditionFailed,
        ),
    ];
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

pub fn focus_editable_text_apply_to_ref<R: RefBlockTreeMut + RefEditorMirrorMut + RefFocusMut>(
    block_id: &EntityUri,
    state: &mut R,
) {
    // Clicking into another editable moves the focus authority away
    // from any currently active editor; prod commits its user-authored
    // pending text on that authority move (focus-binding arm).
    commit_active_editor_if_dirty(state);
    let saved = state
        .block_content(block_id)
        .map(|s| s.to_owned())
        .unwrap_or_default();
    let cursor_byte = saved.len();
    // Open the active editor on this block (replacing any prior editor).
    // Deliberately do not update navigation focus here. While an
    // editor is active, the active editor's block is the source of
    // truth for editor focus and the engine's global `focused_block`
    // is a transient implementation detail. inv-focus-matches-ref is gated on
    // `active_editor.is_none()` to skip exactly this window.
    state.open_active_editor(block_id.clone(), saved, cursor_byte);
}

impl TransitionRef<ReferenceState> for FocusEditableText {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        focus_editable_text_preconditions(&self.block_id, state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        focus_editable_text_apply_to_ref(&self.block_id, state)
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
