//! Transition: type characters into the active editor.
//!
//! @pbt rung input-pipeline
//!   `apply_type_chars` drives editor keystrokes through the production
//!   ReactiveEngineDriver -> HeadlessEditorMirror.
//! @pbt covers editor-typing — character keystrokes -> MutableText edit
//!
//! Mirrors the legacy logic split across `state_machine.rs:1650-1662`
//! (generator), `state_machine.rs:3552-3556` (precondition, shared arm),
//! `state_machine.rs:2961-2964` (ref-state apply),
//! `sut.rs:4409-4418` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefEditorMirrorMut;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutEditorMirrorWrite;
use holon_pbt_core::capabilities::commit_active_editor_if_changed;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;

/// Hops the vocabulary resolver walks for a block sitting directly under its
/// page: the block itself, then the page.
const VOCABULARY_RESOLVE_READS: usize = 2;

/// Type a short ASCII string into the active editor.
/// Gated to `PBT_ATOMIC_EDITOR=1` runs.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TypeChars {
    pub text: String,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────
//
// These are the canonical logic; the `TransitionImpl` below just delegates.
// The pure slice can call these directly without `TransitionImpl`.

/// Preconditions for `TypeChars`, bound only on the capability traits it reads.
pub fn type_chars_preconditions<R: RefEditorMirror + RefFocus + RefLifecycle>(
    state: &R,
) -> Validated<(), Reason> {
    let checks: Vec<Validated<(), Reason>> = vec![
        check(state.has_editor_buffer(), Reason::NoEditorBuffer),
        check(state.app_started(), Reason::AppNotStarted),
        check(state.is_properly_setup(), Reason::NotProperlySetup),
        check(
            state.current_focus(CapRegion::Main).is_some(),
            Reason::NoFocusInMain,
        ),
        check(
            state.active_editor_block().is_some(),
            Reason::NoActiveEditor,
        ),
    ];
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

/// Weighted generator for `TypeChars`, capability-bound.
pub fn type_chars_weighted_generator<R: RefEditorMirror + RefFocus + RefLifecycle>(
    state: &R,
) -> Validated<(u32, BoxedStrategy<TypeChars>), Reason> {
    type_chars_preconditions(state).map(|_| {
        let last = state.last_transition_kind();
        let tc_weight = match last {
            Some("FocusEditableText") | Some("MoveCursor") => 6,
            Some("TypeChars") => 4,
            _ => 1,
        };
        let strat = crate::pbt::generators::typing_text_strategy()
            .prop_map(|text: String| TypeChars { text })
            .boxed();
        (tc_weight, strat)
    })
}

/// Ref-state apply for `TypeChars`, capability-bound: type into the active
/// editor, then commit through to block content.
///
/// ONE KEYSTROKE AT A TIME, deliberately. Prod delivers `text` as N separate
/// keystrokes, each of which runs the whole sink (`apply_local_edit` → commit),
/// and live task-keyword promotion is a function of the DELTA — so a model that
/// applied `text` as one edit would compute a different promotion than the run
/// it is judging. `TODO  milk` (two spaces) is the smallest witness: prod
/// promotes on the FIRST space, when the block is still `TODO `, and the second
/// space is then ordinary text; a one-shot model sees `TODO  milk` whole and
/// trims both spaces away. Everything else here is per-keystroke for the same
/// reason.
pub fn type_chars_apply_to_ref<R>(text: &str, state: &mut R)
where
    R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle,
{
    for ch in text.chars() {
        type_one_char_to_ref(&ch.to_string(), state);
    }
}

/// One keystroke: insert it, then either promote or commit the buffer as
/// ordinary content.
fn type_one_char_to_ref<R>(ch: &str, state: &mut R)
where
    R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle,
{
    let prior_buffer = state.active_editor_text().unwrap_or_default().to_owned();
    state.type_chars(ch);
    // Live task-keyword promotion (task #64): the keystroke that makes a block
    // keyword-headed is an authoring gesture, not text — the keyword becomes
    // `task_state` and leaves the content.
    //
    // ONE decision, on the guard inputs the ENGINE uses: the block's task
    // state, the editor's prior text, and the vocabulary the DRAWN DOCUMENT
    // declares. The vocabulary is derived here rather than copied from prod's
    // constant — a model that hardcodes what prod hardcodes agrees with prod's
    // wrong answer.
    //
    // Prod's trigger cannot see the document, so it proposes on a
    // vocabulary-free shape rule and the engine adjudicates. A refusal is
    // therefore reachable from typing, and the view model's un-strip is what
    // keeps it lossless: both sides land on the typed text either way, which
    // is why this ONE decision still predicts the SUT.
    if let Some(block_id) = state.active_editor_block()
        && let Some(typed) = state.active_editor_text().map(str::to_owned)
        && let Some(caret) = state.active_editor_cursor()
    {
        let prior_state = state
            .block_task_state(&block_id)
            .map(|k| holon_api::TaskState::from_keyword(&k));
        let promotion = holon_org_format::detect_keyword_promotion(
            &prior_buffer,
            prior_state.as_ref(),
            &typed,
            &state.block_task_vocabulary(&block_id),
        );
        if let Some(promotion) = promotion
            && state.promote_block_task_keyword(
                &block_id,
                &promotion.keyword.keyword,
                &promotion.stripped,
            )
        {
            // The keyword left the visible text, so every caret offset moves
            // back by exactly the prefix the promotion consumed.
            state.reseed_active_editor(
                &promotion.stripped,
                caret.saturating_sub(promotion.consumed_prefix),
            );
            state.mark_active_editor_committed();
            return;
        }
    }
    // The GPUI editor now always commits typed text: when Loro is
    // enabled the per-keystroke pipeline writes through the Cell into
    // the Loro doc, and when no cell is attached (SqlOnly / no-Loro
    // mode) the change handler falls back to `set_field("content")`.
    // The ref must mirror this so the invariant sees the same content
    // on both sides regardless of storage backend.
    commit_active_editor_if_changed(state);
}

// ── E2E trait impls (wide PBT entry point; delegate to _cap fns) ──

impl<R: RefEditorMirror + RefFocus + RefLifecycle> TransitionFactory<R> for TypeChars {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // ADR 0009 asymmetry #1: "edit content" works on any block store (under
        // Turso-only via the on-blur `set_field` path), so gate to
        // `AnyStorageOf({Loro, Turso})` — bisectable across the storage axis.
        // Headless Turso-only slices stay unaffected: `preconditions` still
        // requires `enable_loro() || real_editor_enabled()`.
        ::holon_pbt_core::RequiredWiring::any_storage_of([
            ::holon_pbt_core::StorageAdapter::Loro,
            ::holon_pbt_core::StorageAdapter::Turso,
        ])
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        type_chars_weighted_generator(state)
    }
}

impl<R: RefEditorMirror + RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle>
    TransitionRef<R> for TypeChars
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        type_chars_preconditions(state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        type_chars_apply_to_ref(&self.text, state);
    }
}

crate::cap_transition! {
    TypeChars: SutEditorMirrorWrite,
    where R: [ RefEditorMirror + RefFocus + RefLifecycle ],
    |me, _state, sut| {
        sut.apply_type_chars(&me.text).await;
    }
    sql_budget: |me, state| {
        // TypeChars is N keystrokes and EVERY keystroke commits through the
        // editor VM, so the cost is linear in the text — the former flat
        // `REACTIVE_BASE + 10` was measured on short strings only.
        //
        // Dedup reads, samples (chars → reads): Loro 1→7, 5→14, 9→19;
        // SqlOnly 3→12, 9→19; and the promoting draw ("TODO milk") 9→24 in
        // both arms — its middle keystroke reads the block's task keyword,
        // then runs the guard and the compound's two constituents.
        // `REACTIVE_BASE + 2·chars` covers every one with the tolerance
        // UNCHANGED at 5.
        //
        // Writes fork on who holds block CRUD: in Loro the content lands in
        // the CRDT and only the undo journal reaches SQL (chars), in SqlOnly
        // both do (2·chars). The promoting keystroke adds one more write in
        // the SqlOnly arm (measured 19 vs 18) — inside the untouched
        // tolerance, not a widening of it.
        //
        // A keystroke the read gate admits costs the authority one more read
        // per hop from the block to its nearest page ancestor, resolving the
        // owning document's `#+TODO:` vocabulary. Charged ONLY for a
        // candidate-headed text, so the budget still bites on ordinary prose.
        let chars = me.text.chars().count();
        let vocabulary_reads = if holon_org_format::candidate_keyword_headed(&me.text).is_some() {
            VOCABULARY_RESOLVE_READS
        } else {
            0
        };
        ExpectedSql {
            reads: REACTIVE_BASE + 2 * chars + vocabulary_reads,
            writes: if state.content_writes_reach_sql() {
                2 * chars
            } else {
                chars
            },
            ddl: 0,
            tolerance: 5,
        }
    }
}
