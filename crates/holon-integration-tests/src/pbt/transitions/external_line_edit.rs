//! Transition: an external editor saves one edit into a document's org file.
//!
//! @pbt rung external
//!   rewrites the file in the in-memory vault: edits or cuts one headline,
//!   found by its `:ID:` or, in a file without ids, by its title; or drops
//!   the document's own id.
//! @pbt covers write-back-state-machine — a line edited on disk keeps its
//!   block's id, a line deleted on disk leaves the store, and the later
//!   observed of a store edit and a disk edit of one block wins
//!
//! The model applies the edit when the step runs: the ingest is the step at
//! which the store observes it, and no other step runs in between.
//! `StripDocId` has no effect on the model's blocks. An emptied file is
//! refused: the store keeps the document's blocks and discloses the refusal
//! until the user accepts the removal or rewrites the file from the store.

use std::collections::HashMap;

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefApplyMutationMut;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefLayoutInteract;
use holon_pbt_core::capabilities::SutWriteFaults;
use holon_pbt_core::types::ExternalFileEdit;
use holon_pbt_core::types::Mutation;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::arm_write_churn::writeback_faults_enabled;
use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("an external editor saves {edit} into the file of document {doc}")]
pub struct ExternalLineEdit {
    pub doc: EntityUri,
    pub edit: ExternalFileEdit,
}

/// Childless one-line text blocks whose title is unique in their document,
/// so the headline is found by its title even in a file without ids.
fn line_candidates(state: &ReferenceState) -> Vec<(EntityUri, EntityUri, String)> {
    let blocks = &state.domain.block_state.blocks;
    let docs = &state.domain.block_state.block_documents;
    let mut out = Vec::new();
    for (id, block) in blocks {
        let Some(doc) = docs.get(id) else { continue };
        if !state.files.documents.contains_key(doc)
            || state.files.refused.contains_key(doc)
            || block.is_page()
            || blocks.values().any(|b| b.parent_id == *id)
            || state.is_immutable(id)
            || block.content.is_empty()
            || block.content.contains('\n')
            || block.content.contains(':')
            || state.block_task_state(id).is_some()
        {
            continue;
        }
        let same_title = blocks
            .iter()
            .filter(|(other, b)| docs.get(*other) == Some(doc) && b.content == block.content)
            .count();
        if same_title == 1 {
            out.push((doc.clone(), id.clone(), block.content.clone()));
        }
    }
    out
}

/// Documents whose file Holon ingests: not refused as emptied.
fn open_docs(state: &ReferenceState) -> Vec<EntityUri> {
    state
        .files
        .documents
        .keys()
        .filter(|doc| !state.files.refused.contains_key(*doc))
        .cloned()
        .collect()
}

/// Open documents whose write-back no fault holds off, so their file holds
/// the store's content when the editor empties it.
fn still_docs(state: &ReferenceState) -> Vec<EntityUri> {
    open_docs(state)
        .into_iter()
        .filter(|doc| !state.files.write_churn.contains(doc))
        .collect()
}

fn is_candidate(state: &ReferenceState, doc: &EntityUri, block: &EntityUri) -> bool {
    line_candidates(state)
        .iter()
        .any(|(d, b, _)| d == doc && b == block)
}

impl TransitionFactory<ReferenceState> for ExternalLineEdit {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        if !writeback_faults_enabled() {
            return Validated::fail(Reason::HandAuthoredOnly);
        }
        let lines = line_candidates(state);
        let docs = open_docs(state);
        let still = still_docs(state);
        check(
            state.action.app_started && !docs.is_empty(),
            Reason::NoDocumentsAvailable,
        )
        .map(|_| {
            let mut arms: Vec<BoxedStrategy<Self>> = vec![
                (prop::sample::select(docs), "[a-z]{3,8}", any::<bool>())
                    .prop_map(|(doc, title, strip)| ExternalLineEdit {
                        doc,
                        edit: if strip {
                            ExternalFileEdit::StripDocId
                        } else {
                            ExternalFileEdit::AddIdlessHeadline { title }
                        },
                    })
                    .boxed(),
            ];
            if !lines.is_empty() {
                arms.push(
                    (prop::sample::select(lines), any::<bool>(), "[a-z]{3,8}")
                        .prop_map(|((doc, block, disk_text), edit_line, word)| {
                            let edit = if edit_line {
                                ExternalFileEdit::EditLine {
                                    block,
                                    text: format!("{disk_text} {word}"),
                                    disk_text,
                                }
                            } else {
                                ExternalFileEdit::DeleteLine { block, disk_text }
                            };
                            ExternalLineEdit { doc, edit }
                        })
                        .boxed(),
                );
            }
            if !still.is_empty() {
                arms.push(
                    (prop::sample::select(still), any::<bool>())
                        .prop_map(|(doc, whitespace)| ExternalLineEdit {
                            doc,
                            edit: ExternalFileEdit::Empty { whitespace },
                        })
                        .boxed(),
                );
            }
            (6, prop::strategy::Union::new(arms).boxed())
        })
    }
}

impl TransitionRef<ReferenceState> for ExternalLineEdit {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let target_ok = match &self.edit {
            ExternalFileEdit::EditLine { block, .. }
            | ExternalFileEdit::DeleteLine { block, .. } => is_candidate(state, &self.doc, block),
            ExternalFileEdit::StripDocId | ExternalFileEdit::AddIdlessHeadline { .. } => {
                open_docs(state).contains(&self.doc)
            }
            ExternalFileEdit::Empty { .. } => still_docs(state).contains(&self.doc),
        };
        check(
            state.action.app_started && target_ok,
            Reason::PreconditionFailed,
        )
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        match &self.edit {
            ExternalFileEdit::EditLine { block, text, .. } => {
                let fields = HashMap::from([(
                    "content".to_string(),
                    holon_api::Value::String(text.clone()),
                )]);
                state.apply_content_mutation(
                    &Mutation::Update {
                        id: block.clone(),
                        fields,
                    },
                    true,
                );
            }
            ExternalFileEdit::DeleteLine { block, .. } => {
                state.apply_content_mutation(&Mutation::Delete { id: block.clone() }, true);
                state.clear_focus_if_deleted(block);
            }
            ExternalFileEdit::StripDocId => {}
            ExternalFileEdit::AddIdlessHeadline { title } => {
                let id = state.create_block_under_outside_history(&self.doc, title);
                state.files.ingest_origin_blocks.insert(id);
            }
            ExternalFileEdit::Empty { whitespace } => {
                let rows = state.doc_rows(&self.doc);
                state.files.refused.insert(self.doc.clone(), rows);
                let name = crate::pbt::copies_model::file_name_of(state, &self.doc);
                state.conditions.raise_with_message(
                    name,
                    holon_api::ConditionKind::VAULT_FILE_EMPTIED,
                    holon_filesystem::base_sidecar::BaseSidecars::hash(
                        ExternalFileEdit::emptied_text(*whitespace),
                    ),
                );
            }
        }
    }
}

crate::cap_transition! {
    ExternalLineEdit: SutWriteFaults,
    |me, _state, sut| {
        sut.external_file_edit(&me.doc, &me.edit).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for ExternalLineEdit {
    fn expected_sql<R: holon_pbt_core::capabilities::RefSqlCardinality>(
        &self,
        state: &R,
    ) -> ExpectedSql {
        ExpectedSql {
            reads: holon_pbt_core::budget::cdc_drain_floor(state.document_count()) + 60,
            writes: 6,
            ddl: 0,
            tolerance: 48,
        }
    }
}
