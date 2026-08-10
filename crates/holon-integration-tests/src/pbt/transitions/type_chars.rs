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
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I type {text}")]
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
/// keystrokes, each of which runs the whole sink (`apply_local_edit` → commit)
/// and each of which the store canonicalizes, so a model that applied `text` as
/// one edit would judge a state prod never held.
pub fn type_chars_apply_to_ref<R>(text: &str, state: &mut R)
where
    R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle,
{
    for ch in text.chars() {
        type_one_char_to_ref(&ch.to_string(), state);
    }
}

/// One keystroke: insert it into the editable surface, then commit that
/// surface. There is no promotion step — the surface holds vault syntax and the
/// STORE's convergence is the parse, which is the whole shape of arm (d).
fn type_one_char_to_ref<R>(ch: &str, state: &mut R)
where
    R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle,
{
    state.type_chars(ch);
    // The GPUI editor commits every typed keystroke: to `source_text` when the
    // buffer is (or has just stopped being) keyword-headed, to `content`
    // otherwise. `commit_active_editor_if_changed` runs that same routing, so
    // the ref sees the same two columns the SUT does in either storage mode.
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
        // Every keystroke whose buffer is keyword-SHAPED commits through the
        // source channel, and each of those costs the store one read per hop
        // from the block to its nearest page ancestor to resolve the owning
        // document's `#+TODO:` vocabulary. Counted per keystroke rather than
        // per transition — the budget still bites on ordinary prose, which
        // never opens the channel at all.
        let chars = me.text.chars().count();
        let source_keystrokes = (1..=chars)
            .filter(|n| {
                let prefix: String = me.text.chars().take(*n).collect();
                holon_org_format::could_converge(&prefix)
            })
            .count();
        let vocabulary_reads = VOCABULARY_RESOLVE_READS * source_keystrokes;
        ExpectedSql {
            reads: REACTIVE_BASE + 2 * chars + vocabulary_reads,
            // A source-channel keystroke lands TWO columns (content and
            // task_state) where a content keystroke lands one. Charged for
            // every keystroke the channel admits — an OVER-approximation,
            // because the task_state write is skipped while the block carries
            // no keyword to clear; over-charging only loosens a ceiling, and
            // the shape (linear in the keyword-headed prefix) is the point.
            writes: if state.content_writes_reach_sql() {
                2 * chars + source_keystrokes
            } else {
                chars + source_keystrokes
            },
            ddl: 0,
            tolerance: 5,
        }
    }
}
