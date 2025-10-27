//! Transition: split a block at a byte position.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1171-1191`
//! (generator), `state_machine.rs:3426-3436` (precondition),
//! `state_machine.rs:2687-2691` (ref-state apply),
//! `sut.rs:4052-4127` (SUT apply), and
//! `transition_budgets.rs:303-312` (expected SQL).

use std::time::Duration;

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::CapCursor;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefEditorMirrorMut;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefFocusMut;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use holon_pbt_core::capabilities::SutDriver;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::capabilities::commit_active_editor_if_dirty;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

// ── Capability-bound free function (Phase C input pipeline) ──────────

/// SUT-side input-pipeline body of `SplitBlock`. Bound on
/// `SutLayout + SutDriver`. Drives the physical sequence a real user
/// would perform: ensure target is rendered as an editable widget,
/// click to focus, then type `home` + N×`right` + `Enter`.
///
/// The Enter handler at `editor_view.rs:543-575` is a capture_action
/// that reads `input.read(cx).cursor()` from the live `InputState` and
/// dispatches `split_block` against the focused editor — a separate
/// code path from the bubble-phase chord resolver that `Ctrl+x` hits.
/// Driving Enter exercises that production path.
///
/// `wait_for_widget_kind` is a stronger precondition than
/// `wait_for_bounds`: confirms the target is rendered as the
/// interactive `editable_text` or its read-only `rendered_text` sibling
/// (a click can either focus the editor or promote the read-only
/// variant). Mismatches surface here instead of as a confusing focus
/// timeout 1 s later.
///
/// `wait_for_engine_focus` after the click prevents focus drift: if the
/// click silently focuses a different block, Enter would fire against
/// the wrong editor and `split_block` would split the wrong content.
///
/// Caller responsibility: pre-condition gates that depend on
/// pre-transition state (`wait_for_children_settled` on the original
/// parent) and post-transition assertions (block-count sync,
/// synthetic-id mapping). Those stay in the SutHandle adapter because
/// they read from E2ESut-internal state.
/// `right_presses` is a KEYSTROKE count (chars before the split point), not
/// a byte offset — callers convert `SplitBlock.position` (bytes) against the
/// pre-transition content before calling.
pub async fn apply_split_block_input_pipeline_to_sut<S: SutLayout + SutDriver>(
    sut: &S,
    id: &EntityUri,
    right_presses: usize,
) {
    sut.wait_for_widget_kind(
        id,
        &["editable_text", "rendered_text"],
        Duration::from_secs(2),
    )
    .await
    .unwrap_or_else(|e| {
        panic!("[SplitBlock] target {id} not rendered as editable_text/rendered_text: {e}")
    });
    // Click-to-focus with re-click: a legitimate async commit (pending text
    // flushing on a previous editor's authority-leave) can re-render and
    // shift row bounds between bounds resolution and the gpui hit-test, so a
    // single click can land on a neighbor row. Re-resolve fresh bounds and
    // re-click until focus lands; fail loud at the deadline.
    let click_deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    loop {
        // click_entity resolution now rejects content-mask-clipped
        // (degenerate) rects — a row scrolled just outside the viewport
        // resolves to NO center instead of a clip-edge point on a different
        // row. On resolution failure, re-reveal (wait_for_bounds scrolls
        // until the rect is visible) and retry until the deadline.
        if let Err(e) = sut.click_entity(id, "main").await {
            if tokio::time::Instant::now() >= click_deadline {
                panic!("[SplitBlock] click_entity failed for {id}: {e}");
            }
            eprintln!(
                "[SplitBlock] click_entity could not resolve {id} (row likely scrolled out of \
                 view); re-revealing: {e}"
            );
            if let Err(e2) = sut.wait_for_bounds(id, Duration::from_secs(2)).await {
                eprintln!("[SplitBlock] re-reveal of {id} failed (will retry): {e2}");
            }
            continue;
        }
        match sut.wait_for_engine_focus(id, Duration::from_secs(1)).await {
            Ok(()) => break,
            Err(e) => {
                if tokio::time::Instant::now() >= click_deadline {
                    panic!(
                        "[SplitBlock] click_entity did not focus {id} before Enter (after \
                         re-click attempts) — split would have hit the wrong block: {e}"
                    );
                }
                eprintln!(
                    "[SplitBlock] click did not land focus on {id} (re-render likely shifted \
                     bounds mid-click); re-clicking: {e}"
                );
            }
        }
    }
    // Engine focus is set synchronously by the click, but the editor widget
    // mounts (and takes WINDOW focus) only on a following render pass — a
    // keystroke dispatched before that is dropped or, worse, consumed by the
    // previously-focused editor. Gate on the committed frame reporting this
    // editor window-focused.
    sut.wait_for_window_focused_editor(id, Duration::from_secs(2))
        .await
        .unwrap_or_else(|e| {
            panic!("[SplitBlock] {id} never took window focus after click-to-focus: {e}")
        });
    sut.send_raw_keystroke("home", &[])
        .await
        .unwrap_or_else(|e| panic!("[SplitBlock] home failed: {e}"));
    for _ in 0..right_presses {
        sut.send_raw_keystroke("right", &[])
            .await
            .unwrap_or_else(|e| panic!("[SplitBlock] right failed: {e}"));
    }
    sut.send_raw_keystroke("enter", &[])
        .await
        .unwrap_or_else(|e| panic!("[SplitBlock] enter failed: {e}"));
}

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;
use crate::pbt::validation::Reason;
use crate::pbt::validation::check;

/// Split an editable text block at a byte position.
/// Only the currently focused (editable) block is a candidate.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SplitBlock {
    pub block_id: EntityUri,
    pub position: usize,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn split_block_preconditions<R: RefBlockTree + RefLifecycle>(
    block_id: &EntityUri,
    position: usize,
    state: &R,
) -> Validated<(), Reason> {
    let focus_roots = state.focus_root_ids(CapRegion::Main);
    let mut checks: Vec<Validated<(), Reason>> = vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(state.is_properly_setup(), Reason::NotProperlySetup),
        // Block-interaction transitions need the block to render as an
        // interactive widget (ops/draggable) reactively over the navigated
        // focus. Only the default layout does; custom `index.org` query
        // layouts don't (see RefLifecycle::renders_block_interactively).
        check(
            state.renders_block_interactively(block_id),
            Reason::BlocksNotInteractiveUnderLayout,
        ),
    ];
    let content = state.block_content(block_id);
    checks.push(check(content.is_some(), Reason::FocusedBlockMissing));
    checks.push(check(state.is_text_block(block_id), Reason::FocusedNotText));
    if let Some(text) = content {
        checks.push(check(position <= text.len(), Reason::PreconditionFailed));
        // Positions are byte offsets that MUST sit on a char boundary: prod
        // positions come from an editor caret (always boundary-aligned), and
        // both ref + prod `split_block` slice `content[..position]`. A
        // mid-codepoint byte (possible only via hand-edited captures —
        // the generator enumerates `char_indices`) must reject, not panic.
        checks.push(check(
            text.is_char_boundary(position),
            Reason::PreconditionFailed,
        ));
    }
    checks.push(check(
        !state.is_layout_block(block_id),
        Reason::FocusedInLayoutBlocks,
    ));
    checks.push(check(
        state.is_descendant_of_any(block_id, &focus_roots),
        Reason::FocusedNotDescendantOfFocusRoot,
    ));
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

pub fn split_block_weighted_generator<R: RefBlockTree + RefLifecycle>(
    state: &R,
) -> Validated<(u32, BoxedStrategy<SplitBlock>), Reason> {
    let mut candidates: Vec<(EntityUri, usize)> = vec![];
    for id in state.main_editable_descendants() {
        if let Some(text) = state.block_content(&id) {
            // Char boundaries only — a user caret can't sit mid-codepoint.
            // `0..=len` over bytes generated unsliceable positions once
            // multi-byte content existed (extended-gen axis 1).
            for position in text
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(text.len()))
            {
                if split_block_preconditions(&id, position, state).is_good() {
                    candidates.push((id.clone(), position));
                }
            }
        }
    }
    check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
        let strat = prop::sample::select(candidates)
            .prop_map(|(block_id, position)| SplitBlock { block_id, position })
            .boxed();
        // High weight: editing transitions are starved unless Main is
        // populated; SplitBlock exercises the Enter → split path.
        (100, strat)
    })
}

pub fn split_block_apply_to_ref<
    R: RefBlockTreeMut + RefFocusMut + RefEditorMirrorMut + RefFocus,
>(
    block_id: &EntityUri,
    position: usize,
    state: &mut R,
) {
    // Structural ops are commit points (docs/Architecture/UI.md). Two prod
    // paths collapse here: (1) splitting the focused editor flushes its
    // pending text before the split (`dispatch_structural_as_commit_point`);
    // (2) the SUT pipeline's click-to-focus on a DIFFERENT block blurs the
    // previously focused editor, and SqlOnly's on_blur commits its pending
    // text. Both are dirty-gated: only user-authored text commits — a clean
    // mirror that merely diverged from `block.content` is stale against an
    // external change and must NOT be committed (prod's data subscription
    // refreshes idle editors; an unconditional commit here produced the
    // Full/Loro divergence of 2026-06-11).
    commit_active_editor_if_dirty(state);
    state.push_undo_snapshot();
    let new_block_id = state.split_block(block_id, position);
    // Production returns the new block as the focus target (caret 0) in
    // `split_block`'s op response; the frontend applies it in-process
    // (`traits.rs::split_block` → `focus_response`, ADR 0010). Mirror that so
    // subsequent transitions and post-step invariants see the right
    // focused block.
    let new_content = state
        .block_content(&new_block_id)
        .unwrap_or_default()
        .to_string();
    state.set_focus(CapRegion::Main, new_block_id.clone(), CapCursor::default());
    // That focus move also makes the new block the ACTIVE editor at
    // caret 0 — not just the navigation focus. A subsequent `PressKey(Enter)`
    // therefore splits the new block, not the block the prior
    // `FocusEditableText` opened. Without this the ref's `active_editor` stays
    // stale, so PressKey(Enter) re-splits the old target and its content
    // diverges from prod (which left that block untouched) — the
    // settled-consistent `inv-blocks-match-ref` content divergence.
    state.open_active_editor(new_block_id, new_content, 0);
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl<R: RefBlockTree + RefLifecycle> TransitionFactory<R> for SplitBlock {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        // Single-sourced from the `cap_transition!` below — cannot drift with the
        // `S: SutBlockTreeWrite` dispatch bound (both come from the one cap token).
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        split_block_weighted_generator(state)
    }
}

impl<R: RefBlockTree + RefBlockTreeMut + RefFocusMut + RefEditorMirrorMut + RefFocus + RefLifecycle>
    TransitionRef<R> for SplitBlock
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        split_block_preconditions(&self.block_id, self.position, state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        split_block_apply_to_ref(&self.block_id, self.position, state);
    }
}

crate::cap_transition! {
    SplitBlock: SutBlockTreeWrite,
    |me, _state, sut| {
        sut.apply_split_block(&me.block_id, me.position).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for SplitBlock {
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
