//! Transition: move a LEAF block out of the org file that holds it and into
//! Holon's own storage, through the production `block.rehome_entity` op.
//!
//! The op re-parents the block to the no-parent root, which leaves it with no
//! page ancestor and therefore no file. Two facts follow, and both are checked
//! by invariants already in the catalog rather than by this transition:
//! `inv-home-profile-matches-derived` sees the block's profile change from
//! `org` to `holon-native` at the very next check — the binding is derived on
//! read, so it cannot lag a reprojection — and `inv-blocks-match-ref/org` sees
//! the org file stop carrying the block, because write-back re-renders the
//! document it left.
//!
//! @pbt rung dispatch
//!   `rehome_entity` re-parents a leaf to the root via the op-floor
//!   `SutRehomeEntity`, changing which capability profile governs it.
//! @pbt covers rehome-entity — a file-homed leaf moves to Holon's own storage;
//! the derived home profile follows immediately and the old file releases it
//! @pbt covers rehome-machinery-refused — a rule's own source block is drawn
//! and the placement policy refuses it, leaving the rule intact

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::DrawnHome;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefDocumentsMut;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// The home `rehome_entity` moves an entity into. Holon's own store is the only
/// one it accepts — every other home would have to CREATE the block there.
const HOLON_NATIVE: &str = "holon-native";

/// Move `block_id`, a file-homed leaf, into Holon's own storage.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I re-home block {block_id} into Holon storage")]
pub struct RehomeEntity {
    pub block_id: EntityUri,
}

/// Leaves that a tracked file currently holds, INCLUDING rule machinery.
///
/// The op refuses a non-leaf and a block no document holds BY NAME, so
/// offering those would test the refusal rather than the move. Rule machinery
/// is offered: a rule's action block leaving the structure that owns it is a
/// move the placement policy must refuse, and a draw that cannot reach it
/// cannot judge the refusal. Layout scaffolding and display sources stay out —
/// they are the harness's own furniture.
fn candidates<R: RefBlockTree + RefDocuments>(state: &R) -> Vec<EntityUri> {
    state
        .all_non_seed_block_ids()
        .into_iter()
        .filter(|id| !state.is_page_block(id))
        .filter(|id| !state.is_layout_block(id))
        .filter(|id| !state.is_no_content_update(id))
        .filter(|id| !state.is_source_block(id) || state.is_rule_machinery(id))
        .filter(|id| state.sorted_children(id).is_empty())
        .filter(|id| matches!(state.file_home_of(id), DrawnHome::File(_)))
        .collect()
}

impl<R: RefLifecycle + RefBlockTree + RefDocuments> TransitionFactory<R> for RehomeEntity {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates = candidates(state);
        check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|block_id| RehomeEntity { block_id })
                .boxed();
            // Weighted above the ordinary structural transitions: the candidate
            // set is narrow (a file-homed, non-machinery LEAF) and only a
            // frontend-bearing wiring supplies `SutRehomeEntity`, so at the
            // common weights an unforced run draws this zero times and the
            // re-home path is exercised only under HOLON_PBT_WEIGHTS.
            (10, strat)
        })
    }
}

impl<R: RefLifecycle + RefBlockTree + RefBlockTreeMut + RefDocuments + RefDocumentsMut>
    TransitionRef<R> for RehomeEntity
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.block_content(&self.block_id).is_some(),
                Reason::PreconditionFailed,
            ),
            check(
                !state.is_page_block(&self.block_id),
                Reason::PreconditionFailed,
            ),
            check(
                state.sorted_children(&self.block_id).is_empty(),
                Reason::PreconditionFailed,
            ),
            check(
                !state.is_layout_block(&self.block_id)
                    && !state.is_no_content_update(&self.block_id)
                    && (!state.is_source_block(&self.block_id)
                        || state.is_rule_machinery(&self.block_id)),
                Reason::PreconditionFailed,
            ),
            check(
                matches!(state.file_home_of(&self.block_id), DrawnHome::File(_)),
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // Rule machinery does not move: the placement policy refuses it, so the
        // tree, the file, and the home binding are all unchanged.
        if state.is_rule_machinery(&self.block_id) {
            return;
        }
        // Re-parenting to the no-parent root is the whole model: with no page
        // ancestor the block has no file, so `RefDocuments::file_home_of`
        // answers `Storeless` and the derived profile becomes `holon-native`
        // on the very next read. Nothing else needs saying — the draw's own
        // tree IS the home.
        state.push_undo_snapshot();
        state.move_block(&self.block_id, EntityUri::no_parent(), None);
        // …and it moves into Holon's own storage, which is what takes it off
        // disk. Position and storage are separate facts here; a re-home
        // changes both.
        state.rehome_to_native_storage(&self.block_id);
    }
}

crate::cap_transition! {
    RehomeEntity: holon_pbt_core::capabilities::SutRehomeEntity,
    where R: [ RefLifecycle + RefBlockTree + RefDocuments ],
    |me, state, sut| {
        let outcome = sut.rehome_entity(&me.block_id, HOLON_NATIVE).await;
        let expected_refusal = state.is_rule_machinery(&me.block_id);
        match (&outcome, expected_refusal) {
            (holon_pbt_core::capabilities::RehomeOutcome::Moved, false) => {}
            (holon_pbt_core::capabilities::RehomeOutcome::Refused(_), true) => {}
            _ => panic!(
                "re-homing {} : expected {}, got {outcome:?}",
                me.block_id,
                if expected_refusal {
                    "a refusal, because the block is rule machinery"
                } else {
                    "the move to succeed"
                },
            ),
        }
    }
    sql_budget: |_me, state| {
        // One re-parent plus the home walk on either side of it, then the
        // write-back that re-renders the document the block left. Scales with
        // the vault the way every document-mutating op does.
        let blocks = state.block_count();
        ExpectedSql {
            reads: blocks + 12,
            writes: 4,
            ddl: 0,
            tolerance: 24,
        }
    }
}
