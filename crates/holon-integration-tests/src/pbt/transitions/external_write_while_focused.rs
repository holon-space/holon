//! Transition: an external file write landing WHILE a block has an open, DIRTY
//! editor buffer (the EBO seam -- docs/Plans/EditorBufferOwnership-2026-07-20).
//!
//! @pbt rung external
//!   `external_write_while_focused` writes a sibling block into the focused
//!   block's doc (production FileSyncController re-ingest) while an editor is
//!   open with uncommitted user text.
//! @pbt covers external-write-while-focused -- the re-ingest/converge must
//!   NOT clobber the dirty editor buffer (no silent data loss) and must NOT
//!   duplicate blocks. Suspected trigger of the Enter-duplicate cluster
//!   (vault row-292, "external write landing mid-focus").
//!
//! Oracle wiring (both already in the composed catalog):
//!  * `inv-editor-text/mirror` (`active_editor_text`): the reference keeps the
//!    dirty editor's `in_memory_content` (bulk-add does NOT touch the active
//!    editor -- the ruled "dirty editor keeps pending user text; only clean
//!    editors refresh from an external change" policy). If prod's converge
//!    overwrites the dirty buffer with the re-ingested cell, the SUT's
//!    `editor_live_text` diverges from the preserved pending text -> RED = the
//!    data-loss bug.
//!  * `inv-live-children-match-ref`: no duplicate rows from the re-ingest.
//!
//! Reuses `SutSeamMutate::bulk_external_add` verbatim -- the ONLY thing that
//! makes this a distinct class is the precondition forcing a dirty editor open
//! at re-ingest time, a collision `BulkExternalAdd`'s generator almost never
//! hits on its own.

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefLayoutInteract;
use holon_pbt_core::capabilities::RefLayoutMutate;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutSeamMutate;
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

/// Land an external sibling-block write while an editor is dirty-focused.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ExternalWriteWhileFocused {
    pub doc_uri: EntityUri,
    #[serde(with = "holon_api::block::block_wire_vec")]
    pub blocks: Vec<Block>,
}

impl<R: RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate + RefEditorMirror>
    TransitionFactory<R> for ExternalWriteWhileFocused
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Only fire while a DIRTY editor is open -- that's the whole point of
        // the class. `active_editor_dirty` defaults false for lean models.
        let dirty = state.active_editor_dirty();
        let focused = state.active_editor_block();
        let doc = focused
            .as_ref()
            .and_then(|b| state.block_document_of(b))
            .filter(|d| state.has_document_uri(d));
        // OFF by default (same rationale as StaleExternalRewrite); armed by
        // `HOLON_PBT_EXTERNAL_RACES=1`.
        let enabled = std::env::var("HOLON_PBT_EXTERNAL_RACES").is_ok();
        check(enabled && dirty && doc.is_some(), Reason::NoActiveEditor).map(|_| {
            let doc_uri = doc.expect("checked is_some");
            let next_id = state.next_block_id();
            let strat = (
                prop::collection::vec(crate::pbt::generators::bulk_content_strategy(), 1..=3),
                prop::sample::select(vec![doc_uri.clone()]),
            )
                .prop_map(move |(contents, doc_entity_uri)| {
                    let blocks: Vec<Block> = contents
                        .into_iter()
                        .enumerate()
                        .map(|(i, content)| {
                            Block::new_text(
                                EntityUri::block(&format!("ewf-{}-{}", next_id, i)),
                                doc_entity_uri.clone(),
                                content,
                            )
                        })
                        .collect();
                    ExternalWriteWhileFocused {
                        doc_uri: doc_entity_uri,
                        blocks,
                    }
                })
                .boxed();
            (3u32, strat)
        })
    }
}

impl<R: RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate + RefEditorMirror>
    TransitionRef<R> for ExternalWriteWhileFocused
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(state.active_editor_dirty(), Reason::NoActiveEditor),
            check(
                state.has_document_uri(&self.doc_uri),
                Reason::NoDocumentsAvailable,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // Same external bulk-add effect as `BulkExternalAdd`. Crucially,
        // `bulk_add_blocks` does NOT touch `ui.tab.active_editor`, so the dirty
        // editor's pending `in_memory_content` is PRESERVED -- the ruled
        // "external change refreshes clean editors only" policy. The SUT must
        // match: dirty buffer survives the re-ingest.
        state.bulk_add_blocks(&self.doc_uri, &self.blocks);
    }
}

crate::cap_transition! {
    ExternalWriteWhileFocused: SutSeamMutate,
    where R: [ RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate + RefEditorMirror ],
    |me, _state, sut| {
        sut.bulk_external_add(&me.doc_uri, &me.blocks).await;
    }
    sql_budget: |me, state| {
        let n = me.blocks.len();
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
        ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + 1 + n + watches * READS_PER_WATCH,
            writes: n + 2,
            ddl: watches,
            tolerance: cdc_tolerance(blocks + n, docs) + n * 3,
        }
    }
}
