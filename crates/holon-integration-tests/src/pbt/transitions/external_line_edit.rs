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
//! `StripDocId` has no effect on the model's blocks.

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
        check(
            state.action.app_started && !lines.is_empty(),
            Reason::NoDocumentsAvailable,
        )
        .map(|_| {
            let strat = (prop::sample::select(lines), 0..3u8, "[a-z]{3,8}")
                .prop_map(|((doc, block, disk_text), op, word)| {
                    let edit = match op {
                        0 => ExternalFileEdit::EditLine {
                            block,
                            text: format!("{disk_text} {word}"),
                            disk_text,
                        },
                        1 => ExternalFileEdit::DeleteLine { block, disk_text },
                        _ => ExternalFileEdit::StripDocId,
                    };
                    ExternalLineEdit { doc, edit }
                })
                .boxed();
            (6, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for ExternalLineEdit {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let target_ok = match &self.edit {
            ExternalFileEdit::EditLine { block, .. }
            | ExternalFileEdit::DeleteLine { block, .. } => is_candidate(state, &self.doc, block),
            ExternalFileEdit::StripDocId => state.files.documents.contains_key(&self.doc),
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
