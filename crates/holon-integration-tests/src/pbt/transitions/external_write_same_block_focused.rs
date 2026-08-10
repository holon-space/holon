//! Transition: a SUBSTANTIVE external content write landing on the block whose
//! editor is CURRENTLY FOCUSED, while that editor's live buffer DIVERGES from
//! the committed projection (mid-edit) — the external-write-while-focused seam
//! (docs/Testing/Keystone PBT.org "External-write-while-focused transition").
//!
//! OFF by default; armed by `HOLON_PBT_EXTERNAL_RACES=1` (same gate as
//! `ExternalWriteWhileFocused` / `StaleExternalRewrite`). The whole-doc org
//! re-ingest it drives trips the PARKED reconcile-duplication red (a re-minted
//! duplicate of a short/non-unique-content block surfaces as an
//! `inv-matview-consistent-with-recompute` watch-view drift — the PR #81 /
//! StaleExternalRewrite family, NOT the echo decision), so it must not red the
//! shared keystone. Armed, it engages the echo/convergence oracle below AND is
//! the headless vehicle for that reconcile red.
//!
//! @pbt rung external
//!   `external_write_same_block_focused` rewrites the focused block's content
//!   through the production FileSyncController re-ingest path (org-file
//! rewrite,   the same `SutSeamMutate::apply_mutation` seam the composed
//! `ApplyMutation`   External arm drives), targeting the EXACT block the user
//! is editing — a   collision the generic `ApplyMutation` generator (which
//! picks any text   block) practically never hits on its own.
//! @pbt covers external-write-while-focused-same-block — the ruled data-sync
//!   echo/convergence decision
//! (`holon_frontend::echo::evaluate_data_sync_echo`,   whose 13 unit tests ARE
//! the spec) applied on the LIVE focused editor:   a substantive external
//! change to the focused block must CONVERGE the buffer   to the new authority
//! (the `Converge` arm — non-editor writers do not bump   `write_seq`, so the
//! echo carries `seq == last_local` and the trim-shape   discriminator, not the
//! seq, decides), while the block's OWN   trailing-whitespace canonicalization
//! keeps the typed buffer   (`AdoptBaseline`). Suspected trigger of the
//! Enter-duplicate / "typing   resets the block" cluster (BugFunnel rows
//! 292/322/334).
//!
//! Oracle wiring (both already in the composed catalog):
//!  * `inv-editor-text/mirror` (`active_editor_text`): after the re-ingest the
//!    reference applies the SAME echo decision the SUT's `converge_editor` runs
//!    (`RefApplyMutationMut::apply_content_mutation` →
//!    `refresh_clean_active_editor`, whose trim-shape discriminator mirrors
//!    `evaluate_data_sync_echo`). If prod's converge diverges from the ruled
//!    decision — clobbering a buffer it should keep, or keeping one it should
//!    converge — the SUT's `editor_live_text` diverges from the reference
//!    buffer → RED = the echo-guard defect.
//!  * `inv-editor-caret/mirror` + `inv-live-children-match-ref`: the focused
//!    re-ingest must not scramble the caret mirror or duplicate the block.
//!
//! Reuses `RefApplyMutationMut::apply_content_mutation` (the reference side of
//! `ApplyMutation`) and `SutSeamMutate::apply_mutation` (its composed External
//! seam) verbatim — the ONLY new thing is the precondition forcing the write
//! onto the focused, buffer-diverged block (extension, not rewrite).

use holon_api::EntityUri;
use holon_api::Value;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefApplyMutationMut;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutSeamMutate;
use holon_pbt_core::types::Mutation;
use holon_pbt_core::types::MutationEvent;
use holon_pbt_core::types::MutationSource;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::CACHE_EVENT_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::READS_PER_WATCH;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::cdc_tolerance;

/// Rewrite the currently-focused block's content through the external
/// (org-file re-ingest) channel, colliding with the open editor.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("an external edit sets the focused block {block_id} to {new_content}")]
pub struct ExternalWriteSameBlockFocused {
    pub block_id: EntityUri,
    pub new_content: String,
}

impl ExternalWriteSameBlockFocused {
    fn event(&self) -> MutationEvent {
        let fields = [(
            "content".to_string(),
            Value::String(self.new_content.clone()),
        )]
        .into_iter()
        .collect();
        MutationEvent {
            source: MutationSource::External,
            mutation: Mutation::Update {
                id: self.block_id.clone(),
                fields,
            },
        }
    }
}

impl<R: RefLifecycle + RefEditorMirror + RefBlockTree + RefDocuments> TransitionFactory<R>
    for ExternalWriteSameBlockFocused
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Fire only while a text-block editor is focused AND its owning doc has a
        // real file (the org-rewrite seam looks the file up). A page/source block
        // or a fileless doc has no external channel, so the class narrows out.
        let focused = state.active_editor_block();
        let target = focused.filter(|b| {
            state.is_text_block(b)
                && !state.is_page_block(b)
                && state.block_content(b).is_some()
                && state
                    .block_document_of(b)
                    .is_some_and(|d| state.has_document_uri(&d))
        });
        // OFF by default (same rationale as `ExternalWriteWhileFocused` /
        // `StaleExternalRewrite`): the whole-doc external-rewrite + re-ingest seam
        // this transition drives also trips the PARKED reconcile-duplication red
        // (a re-minted duplicate of a short/non-unique-content block surfaces as
        // an `inv-matview-consistent-with-recompute` watch-view drift), so it must
        // not red the shared keystone. `HOLON_PBT_EXTERNAL_RACES=1` arms it for
        // the targeted echo/reconcile run.
        let enabled = std::env::var("HOLON_PBT_EXTERNAL_RACES").is_ok();
        check(
            enabled && state.app_started() && target.is_some(),
            Reason::NoActiveEditor,
        )
        .map(|_| {
            let block_id = target.expect("checked is_some");
            // Divergence window: buffer ahead of the committed projection (the
            // mid-edit "uncommitted local typing" the class is about — reached
            // headless via a trailing-whitespace keystroke the store trims).
            // When it is OPEN the external write races a diverged buffer (the
            // hazard), so weight it up like `PressKey`'s pending-edit arm; when
            // closed the collision is still valid, lower-value coverage.
            let diverged = match (state.active_editor_text(), state.block_content(&block_id)) {
                (Some(buf), Some(committed)) => buf != committed,
                _ => false,
            };
            let weight = if diverged { 8 } else { 2 };
            let strat = (prop::sample::select(vec![block_id]), "ext-[a-z]{3,6}")
                .prop_map(|(block_id, new_content)| ExternalWriteSameBlockFocused {
                    block_id,
                    new_content,
                })
                .boxed();
            (weight, strat)
        })
    }
}

impl<R: RefLifecycle + RefBlockTree + RefApplyMutationMut> TransitionRef<R>
    for ExternalWriteSameBlockFocused
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.block_content(&self.block_id).is_some(),
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // The SAME reference effect as the composed `ApplyMutation` External arm:
        // cross the org-file boundary (write + re-ingest), so the ref models the
        // file-parse tag reinterpretation AND — crucially — runs the focused
        // editor through `refresh_clean_active_editor`'s trim-shape echo
        // discriminator (the reference twin of prod's `converge_editor` /
        // `evaluate_data_sync_echo`). A substantive change converges the buffer;
        // the buffer's own trailing-whitespace canonicalization keeps it.
        state.apply_content_mutation(&self.event().mutation, true);
    }
}

crate::cap_transition! {
    ExternalWriteSameBlockFocused: SutSeamMutate,
    where R: [ RefLifecycle + RefBlockTree + RefApplyMutationMut ],
    |me, _state, sut| {
        sut.apply_mutation(me.event()).await;
    }
    sql_budget: |_me, state| {
        // Org-rewrite + re-ingest reconciles the whole doc; the oracle of record
        // is structural (editor buffer + block tree), not SQL cardinality, so the
        // budget stays permissive (mirrors the other external-seam transitions).
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
        ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + 1 + watches * READS_PER_WATCH,
            writes: 3,
            ddl: watches,
            tolerance: cdc_tolerance(blocks, docs) + blocks * 4 + 32,
        }
    }
}
