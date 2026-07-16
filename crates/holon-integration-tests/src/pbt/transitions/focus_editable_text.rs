//! Transition: focus an editable text block in the main region.
//!
//! @pbt rung input-pipeline
//!   `apply_focus_editable_text_to_sut`: click_entity(main) through the
//!   production driver (editable_text binds no intent -> falls through to
//!   in-memory set_focus, ADR 0010).
//! @pbt covers focus-editor-click — main-panel click -> editor focus
//!
//! Mirrors the legacy logic split across `state_machine.rs:1563-1628`
//! (generator), `state_machine.rs:3529-3550` (precondition),
//! `state_machine.rs:2935-2955` (ref-state apply),
//! `sut.rs:4343-4394` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use std::time::Duration;

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefEditorMirrorMut;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefFocusMut;
use holon_pbt_core::capabilities::RefFocusRoots;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutDriver;
use holon_pbt_core::capabilities::SutFocusWrite;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::capabilities::commit_active_editor_if_dirty;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;

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

impl<
    R: RefLifecycle
        + RefLayout
        + RefFocus
        + RefFocusMut
        + RefEditorMirror
        + RefEditorMirrorMut
        + RefBlockTree
        + RefBlockTreeMut
        + RefFocusRoots,
> TransitionFactory<R> for FocusEditableText
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
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

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (text blocks in main panel) and let
        // `preconditions` be the single source of truth for which ones are
        // actually focusable. Avoids duplicating the content_type / layout /
        // focusable / page checks across two sites.
        let candidates: Vec<EntityUri> = state
            .all_block_ids()
            .into_iter()
            .filter(|uri| {
                FocusEditableText {
                    block_id: uri.clone(),
                }
                .preconditions(state)
                .is_good()
            })
            .collect();

        check(!candidates.is_empty(), Reason::NoFocusableBlocks).map(|_| {
            let last = state.last_transition_kind();
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

impl<
    R: RefLifecycle
        + RefFocus
        + RefFocusMut
        + RefEditorMirror
        + RefEditorMirrorMut
        + RefBlockTree
        + RefBlockTreeMut
        + RefFocusRoots,
> TransitionRef<R> for FocusEditableText
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
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
                state.renders_block_interactively(&self.block_id),
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
                state.is_text_block(&self.block_id) && !state.is_page_block(&self.block_id),
                Reason::FocusedNotText,
            ),
            check(
                !state.is_layout_block(&self.block_id),
                Reason::FocusedInLayoutBlocks,
            ),
            check(
                state.is_descendant_of_any(&self.block_id, &focus_roots),
                Reason::FocusedNotDescendantOfFocusRoot,
            ),
            check(
                !state.is_no_content_update(&self.block_id),
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // Clicking into another editable moves the focus authority away
        // from any currently active editor; prod commits its user-authored
        // pending text on that authority move (focus-binding arm).
        commit_active_editor_if_dirty(state);
        let saved = state
            .block_content(&self.block_id)
            .map(str::to_string)
            .unwrap_or_default();
        let cursor_byte = saved.len();
        // Open a fresh (clean) active editor on the target at end-of-content.
        state.open_active_editor(self.block_id.clone(), saved, cursor_byte);
        // Deliberately do not update navigation focus here. While an
        // editor is active, `active_editor.block_id` is the source of
        // truth for editor focus and the engine's global `focused_block`
        // is a transient implementation detail (the click handler may or
        // may not propagate through to the ui_state mirror depending on
        // whether the GPUI window has fully painted). inv-focus-matches-ref is
        // gated on `active_editor.is_none()` to skip exactly this
        // window.
    }
}

crate::cap_transition! {
    FocusEditableText: SutFocusWrite,
    where R: [ RefLifecycle ],
    |me, _state, sut| {
        sut.apply_focus_editable_text(&me.block_id).await;
    }
    sql_budget: |_me, _state| {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}
