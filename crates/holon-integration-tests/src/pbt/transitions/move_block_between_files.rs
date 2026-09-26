//! Transitions: an external editor cuts a heading (with its subtree) out of
//! one org page file and pastes it into another.
//!
//! @pbt rung external
//!   rewrites page files through the in-memory vault and lets the production
//!   FileSyncController ingest them.
//! @pbt covers move-block-between-files — no save order loses the block; a
//!   block held by two files at once stays in both, is disclosed naming both,
//!   and is resolved by the later save (D229.b)
//!
//! `MoveBlockBetweenFiles` saves both files back to back. `PasteBlockCopy`
//! saves only the target, so the invariants run while both files hold the
//! block (`crate::pbt::copies_model`); `FinishCutPaste` later saves the source
//! or deletes the copy, with Holon's edits in between.

use holon_api::EntityUri;
use holon_orgmode::OrgDocumentExt;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::DrawnHome;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutSeamMutate;
use holon_pbt_core::types::CutPasteSaveOrder;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::block_state::Placement;
use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::cdc_tolerance;

/// Move `block_id` from `source_doc`'s file to the end of `target_doc`'s file.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template(
    "an external editor moves block {block_id} from document {source_doc} to document \
     {target_doc}, saving {order}"
)]
pub struct MoveBlockBetweenFiles {
    pub block_id: EntityUri,
    pub source_doc: EntityUri,
    pub target_doc: EntityUri,
    pub order: CutPasteSaveOrder,
}

/// A page file the layout does not own and no share governs.
fn movable_doc(state: &ReferenceState, doc: &EntityUri) -> bool {
    state
        .files
        .documents
        .get(doc)
        .is_some_and(|name| name != "index.org")
        && !state.sharing.page_shares.contains_key(doc)
}

/// The heading's task keywords must mean the same in both files, or the paste
/// would re-parse them as heading text.
fn same_keywords(state: &ReferenceState, a: &EntityUri, b: &EntityUri) -> bool {
    let keywords = |doc: &EntityUri| {
        state
            .domain
            .block_state
            .blocks
            .get(doc)
            .and_then(|page| page.todo_keywords())
    };
    keywords(a) == keywords(b)
}

/// A plain text subtree: no page, no source block, nothing the layout or a
/// peer owns.
fn movable_block(state: &ReferenceState, id: &EntityUri) -> bool {
    let peer_modified = state.peer_modified_stable_ids();
    state
        .domain
        .block_state
        .subtree_ids(id)
        .iter()
        .all(|member| {
            state.is_text_block(member)
                && !state.is_page_block(member)
                && !state.is_layout_block(member)
                && !state.is_no_content_update(member)
                && !peer_modified.contains(member.id())
                && !state.sharing.policy_audience.contains_key(member)
        })
}

/// The cut & paste moves of a block no copy involves, from its file to
/// another page file.
pub(super) fn candidates(state: &ReferenceState) -> Vec<(EntityUri, EntityUri, EntityUri)> {
    let docs: Vec<EntityUri> = state
        .files
        .documents
        .keys()
        .filter(|doc| movable_doc(state, doc))
        .cloned()
        .collect();
    let mut out = Vec::new();
    for id in state.all_non_seed_block_ids() {
        let DrawnHome::File(source) = state.file_home_of(&id) else {
            continue;
        };
        if !docs.contains(&source)
            || !owned_by_its_file(state, &id, &source)
            || !movable_block(state, &id)
            || involves_a_copy(state, &id)
        {
            continue;
        }
        for target in docs.iter().filter(|t| **t != source) {
            if same_keywords(state, &source, target) {
                out.push((id.clone(), source.clone(), target.clone()));
            }
        }
    }
    out
}

impl TransitionFactory<ReferenceState> for MoveBlockBetweenFiles {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    /// The copy rules write the owner's and the copy's files back; without
    /// the org write-back leg the owner's disk is not the store's.
    fn required_wiring() -> holon_pbt_core::RequiredWiring {
        holon_pbt_core::RequiredWiring::HasStorage(holon_pbt_core::StorageAdapter::Org)
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates = candidates(state);
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!candidates.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            let strat = (
                prop::sample::select(candidates),
                prop::sample::select(CutPasteSaveOrder::ALL.to_vec()),
            )
                .prop_map(
                    |((block_id, source_doc, target_doc), order)| MoveBlockBetweenFiles {
                        block_id,
                        source_doc,
                        target_doc,
                        order,
                    },
                )
                .boxed();
            (250, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for MoveBlockBetweenFiles {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        preconditions_of_a_cut(state, &self.block_id, &self.source_doc, &self.target_doc)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let in_target_file = state
            .domain
            .block_state
            .blocks
            .values()
            .map(|b| b.id.clone())
            .filter(|sibling| {
                *sibling != self.block_id
                    && state.file_home_of(sibling) == DrawnHome::File(self.target_doc.clone())
                    && !state.domain.block_state.is_off_disk(sibling)
            })
            .collect();
        move_in_ref(
            state,
            &self.block_id,
            &self.target_doc,
            &Placement::LastInFile(in_target_file),
        );
    }
}

/// Re-parent `block_id`'s subtree into `target_doc`, in the live state and in
/// every undo/redo snapshot: an ingest is not an undoable op.
pub fn move_in_ref(
    state: &mut ReferenceState,
    block_id: &EntityUri,
    target_doc: &EntityUri,
    placement: &Placement,
) {
    state
        .domain
        .block_state
        .move_subtree_to_document(block_id, target_doc, placement);
    for snapshot in state
        .action
        .undo_stack
        .iter_mut()
        .chain(state.action.redo_stack.iter_mut())
    {
        snapshot.move_subtree_to_document(block_id, target_doc, placement);
    }
    state.rebuild_profile_tracking();
}

/// Whether the page that owns `id` is `doc` itself, whose file holds it, and
/// not a page without a file of its own nested inside `doc`.
fn owned_by_its_file(state: &ReferenceState, id: &EntityUri, doc: &EntityUri) -> bool {
    state.owning_page(id).is_some_and(|page| page.id == *doc)
}

/// Pastes of a copied heading into one more page file, from its owner's
/// file.
pub(super) fn another_copy_candidates(
    state: &ReferenceState,
) -> Vec<(EntityUri, EntityUri, EntityUri)> {
    let mut out = Vec::new();
    for (id, copy) in &state.files.copies {
        if copy.conflict || !copy.undone.is_empty() || !movable_block(state, id) {
            continue;
        }
        let Some(owner) = state.owner_doc_of(id) else {
            continue;
        };
        for target in state.files.documents.keys() {
            if *target != owner
                && !copy.files.contains_key(target)
                && movable_doc(state, target)
                && same_keywords(state, &owner, target)
            {
                out.push((id.clone(), owner.clone(), target.clone()));
            }
        }
    }
    out
}

/// Whether a copy holds `id`, a block above it, or a block below it.
pub(super) fn involves_a_copy(state: &ReferenceState, id: &EntityUri) -> bool {
    state.copied_heading_above(id).is_some()
        || state
            .domain
            .block_state
            .subtree_ids(id)
            .iter()
            .any(|member| state.files.copies.contains_key(member))
}

pub(super) fn preconditions_of_a_cut(
    state: &ReferenceState,
    block_id: &EntityUri,
    source_doc: &EntityUri,
    target_doc: &EntityUri,
) -> Validated<(), Reason> {
    vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(
            state.block_content(block_id).is_some()
                && state.file_home_of(block_id) == DrawnHome::File(source_doc.clone())
                && owned_by_its_file(state, block_id, source_doc)
                && movable_block(state, block_id)
                && !involves_a_copy(state, block_id),
            Reason::PreconditionFailed,
        ),
        check(
            source_doc != target_doc
                && movable_doc(state, source_doc)
                && movable_doc(state, target_doc)
                && same_keywords(state, source_doc, target_doc),
            Reason::PreconditionFailed,
        ),
    ]
    .into_iter()
    .collect::<Validated<Vec<()>, _>>()
    .map(|_| ())
}

crate::cap_transition! {
    MoveBlockBetweenFiles: SutSeamMutate,
    where R: [ RefLifecycle + RefBlockTree + RefDocuments ],
    |me, _state, sut| {
        sut.move_block_between_files(&me.block_id, &me.source_doc, &me.target_doc, me.order)
            .await;
    }
    sql_budget: |_me, state| {
        // Two file ingests, each re-reading its document and re-rendering it.
        let blocks = state.block_count();
        let docs = state.document_count();
        ExpectedSql {
            reads: 2 * (holon_pbt_core::budget::cdc_drain_floor(docs) + 22) + 30,
            writes: 8,
            ddl: 0,
            tolerance: cdc_tolerance(blocks, docs) + 32,
        }
    }
}
