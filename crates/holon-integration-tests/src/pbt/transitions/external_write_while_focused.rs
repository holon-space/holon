//! Transition: an external file write landing WHILE the focused block's editor
//! buffer DIVERGES from its committed projection (the EBO seam --
//! docs/Plans/EditorBufferOwnership-2026-07-20).
//!
//! The divergence window is the same one `ExternalWriteSameBlockFocused` fires
//! in, and the same one prod holds: prod commits every keystroke synchronously
//! (`frontends/gpui/src/views/editor_view.rs:441` dispatches
//! `apply_local_edit` on every `InputEvent::Change`; no debounce, no deferred
//! commit), so the buffer is only ever ahead of the projection by what the
//! store canonicalizes away -- a trailing-whitespace keystroke the store
//! trims. `pending_buffer` (`transitions/mod.rs`) compares the buffer with the
//! source projection the editor was mounted on, which is what the commit path
//! compares too.
//! The reference's `active_editor_dirty` flag is NOT that window: the only ref
//! path that leaves it set is mid-line Backspace
//! (`transitions/press_key.rs:264`), which prod and the headless SUT both
//! commit (`holon-frontend/src/headless_editor_mirror.rs:775`), so gating on it
//! made this class unreachable in any state prod can hold.
//!
//! @pbt rung external
//!   `external_write_while_focused` writes a sibling block into the focused
//!   block's doc (production FileSyncController re-ingest) while an editor is
//!   open with a buffer ahead of the committed projection.
//! @pbt covers external-write-while-focused -- the re-ingest/converge must
//!   NOT clobber the pending editor buffer (no silent data loss) and must NOT
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
//! makes this a distinct class is the precondition forcing a focused editor
//! with a pending buffer at re-ingest time, a collision `BulkExternalAdd`'s
//! generator almost never hits on its own.

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
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
use crate::pbt::transitions::pending_buffer;

/// Land an external sibling-block write while an editor is dirty-focused.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("an external write of {blocks} lands on document {doc_uri} while focused")]
pub struct ExternalWriteWhileFocused {
    pub doc_uri: EntityUri,
    #[serde(with = "holon_api::block::block_wire_vec")]
    pub blocks: Vec<Block>,
}

impl<
    R: RefLifecycle
        + RefDocuments
        + RefLayoutInteract
        + RefLayoutMutate
        + RefEditorMirror
        + RefBlockTree,
> TransitionFactory<R> for ExternalWriteWhileFocused
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Only fire while the focused editor's buffer is ahead of its committed
        // projection -- that's the whole point of the class.
        let pending = pending_buffer(state);
        let focused = state.active_editor_block();
        let doc = focused
            .as_ref()
            .and_then(|b| state.block_document_of(b))
            .filter(|d| state.has_document_uri(d));
        // OFF by default (same rationale as StaleExternalRewrite); armed by
        // `HOLON_PBT_EXTERNAL_RACES=1`.
        let enabled = std::env::var("HOLON_PBT_EXTERNAL_RACES").is_ok();
        check(enabled && pending && doc.is_some(), Reason::NoActiveEditor).map(|_| {
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

impl<
    R: RefLifecycle
        + RefDocuments
        + RefLayoutInteract
        + RefLayoutMutate
        + RefEditorMirror
        + RefBlockTree,
> TransitionRef<R> for ExternalWriteWhileFocused
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(pending_buffer(state), Reason::NoActiveEditor),
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
        // `bulk_add_blocks` does NOT touch `ui.tab.active_editor`, so the
        // editor's pending `in_memory_content` is PRESERVED -- the ruled
        // "external change refreshes clean editors only" policy. The SUT must
        // match: dirty buffer survives the re-ingest.
        state.bulk_add_blocks(&self.doc_uri, &self.blocks);
    }
}

crate::cap_transition! {
    ExternalWriteWhileFocused: SutSeamMutate,
    where R: [ RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate + RefEditorMirror + RefBlockTree ],
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

#[cfg(test)]
mod tests {
    use holon_pbt_core::capabilities::RefLayout;

    use super::*;
    use crate::pbt::composed::wide_e2e::wide_e2e_ref;
    use crate::pbt::ui_types::ActiveEditor;

    /// A seeded non-empty text block together with its document, plus an
    /// external write aimed at that document.
    fn seeded_text_block() -> (
        crate::pbt::reference_state::ReferenceState,
        EntityUri,
        EntityUri,
    ) {
        let state = wide_e2e_ref();
        let (id, doc_uri) = state
            .all_block_ids()
            .into_iter()
            .find_map(|b| {
                let doc = state.block_document_of(&b)?;
                (state.has_document_uri(&doc)
                    && state.block_content(&b).is_some_and(|c| !c.is_empty()))
                .then_some((b, doc))
            })
            .expect("wide_e2e_ref seeds a non-empty text block inside a document");
        (state, id, doc_uri)
    }

    fn probe(doc_uri: &EntityUri) -> ExternalWriteWhileFocused {
        ExternalWriteWhileFocused {
            doc_uri: doc_uri.clone(),
            blocks: vec![Block::new_text(
                EntityUri::block("ewf-probe"),
                doc_uri.clone(),
                "external sibling".to_string(),
            )],
        }
    }

    /// The admission predicate is the DIVERGENCE between the focused buffer and
    /// the committed projection, not a ref-only dirty flag: prod commits every
    /// keystroke synchronously (`editor_view.rs`'s `InputEvent::Change` arm),
    /// so the only window in which a focused buffer is ahead of its
    /// projection is what the store canonicalizes away. A buffer equal to
    /// the projection must REFUSE the class -- otherwise it races a state
    /// prod never holds; a buffer carrying the trailing space the store
    /// trims must ADMIT it. Both halves set `dirty` the WRONG way round, so
    /// only the new predicate can pass.
    #[test]
    fn admission_tracks_the_buffer_projection_divergence() {
        let (mut state, id, doc_uri) = seeded_text_block();
        let stored = state
            .block_content(&id)
            .expect("checked is_some")
            .to_string();

        state.ui.tab.active_editor = Some(ActiveEditor {
            block_id: id.clone(),
            in_memory_content: stored.clone(),
            cursor_byte: stored.len(),
            dirty: true,
        });
        assert!(
            probe(&doc_uri).preconditions(&state).is_fail(),
            "a committed buffer is not the mid-edit window, whatever `dirty` says"
        );

        let pending = format!("{stored} ");
        state.ui.tab.active_editor = Some(ActiveEditor {
            block_id: id.clone(),
            in_memory_content: pending.clone(),
            cursor_byte: pending.len(),
            dirty: false,
        });
        assert!(
            probe(&doc_uri).preconditions(&state).is_good(),
            "the trailing space the store trims IS the window, whatever `dirty` says"
        );
    }

    /// A CLEAN, just-focused editor on a KEYWORD-HEADED block: nothing was
    /// typed, so the class must refuse. The editor is mounted on the source
    /// projection (`TODO x`) while `block_content` is `x`, so comparing
    /// against the content admits this state forever -- the very failure mode
    /// gating on `active_editor_dirty` had.
    #[test]
    fn clean_focus_on_a_keyword_headed_block_is_refused() {
        use holon_pbt_core::capabilities::RefBlockTreeMut;

        use crate::pbt::transitions::focus_editable_text::FocusEditableText;
        let (mut state, id, doc_uri) = seeded_text_block();
        state.set_block_task_state(&id, Some("TODO"));
        FocusEditableText {
            block_id: id.clone(),
        }
        .apply_to_ref(&mut state);
        assert_ne!(
            state.active_editor_text().map(str::to_string),
            state.block_content(&id).map(str::to_string),
            "the keyword prefix must really be on the surface, or this proves nothing"
        );
        assert!(
            probe(&doc_uri).preconditions(&state).is_fail(),
            "a clean editor just focused on a task block has nothing pending"
        );
    }
}
