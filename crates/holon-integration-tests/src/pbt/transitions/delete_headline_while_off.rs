//! Transition: an external editor deletes a headline from its org file while
//! the app is stopped, then the app boots again.
//!
//! @pbt rung external
//!   shuts the session down, cuts the headline's section out of the file
//!   through the in-memory vault, and boots again over the same store
//!   (`SutAppLifecycle::reboot_after_deleting_headline`).
//! @pbt covers reboot-persistence — a headline deleted on disk while the app
//!   is off is gone from the store and from the file after the boot

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefApplyMutationMut;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefReboot;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::types::Mutation;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// The oracle is the one an `External` delete has, applied across a boot:
/// the block and its subtree are gone from the store and from the file, and
/// the in-memory state resets as on [`super::Reboot`].
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template(
    "while the app is stopped, an external editor deletes block {block_id} from the file of \
     document {doc}, and the app boots again"
)]
pub struct DeleteHeadlineWhileOff {
    pub block_id: EntityUri,
    pub doc: EntityUri,
}

fn candidates(state: &ReferenceState) -> Vec<DeleteHeadlineWhileOff> {
    let mut out = Vec::new();
    for id in state.all_block_ids() {
        if !state.domain.block_state.blocks[&id]
            .content_type
            .is_heading()
            || state.is_page_block(&id)
            || state.is_layout_block(&id)
        {
            continue;
        }
        let Some(doc) = state.block_document_of(&id) else {
            continue;
        };
        if !state.has_document_uri(&doc) {
            continue;
        }
        out.push(DeleteHeadlineWhileOff { block_id: id, doc });
    }
    out
}

impl TransitionFactory<ReferenceState> for DeleteHeadlineWhileOff {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> holon_pbt_core::RequiredWiring {
        holon_pbt_core::RequiredWiring::HasStorage(holon_pbt_core::StorageAdapter::Turso)
    }

    /// Gated and weighted with [`super::Reboot`]: it is a reboot.
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let enabled = std::env::var("HOLON_PBT_REBOOT").is_ok();
        let candidates = candidates(state);
        let checks: Vec<Validated<(), Reason>> = vec![
            check(enabled, Reason::PreconditionFailed),
            check(state.app_started(), Reason::AppNotStarted),
            check(!candidates.is_empty(), Reason::PreconditionFailed),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| {
                (
                    super::reboot::reboot_weight(),
                    prop::sample::select(candidates).boxed(),
                )
            })
    }
}

impl TransitionRef<ReferenceState> for DeleteHeadlineWhileOff {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                candidates(state)
                    .iter()
                    .any(|c| c.block_id == self.block_id && c.doc == self.doc),
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.apply_content_mutation(
            &Mutation::Delete {
                id: self.block_id.clone(),
            },
            true,
        );
        state.clear_focus_if_deleted(&self.block_id);
        state.reboot_drops_in_memory_state();
    }
}

crate::cap_transition! {
    DeleteHeadlineWhileOff: SutAppLifecycle,
    where R: [ RefLifecycle + RefBlockTree + RefDocuments ],
    |me, _state, sut| {
        // The composed harness intercepts this like `Reboot`
        // (`ComposedSlice::is_reboot`); this arm serves the non-composed ones.
        sut.reboot_after_deleting_headline(&me.block_id, &me.doc).await;
    }
    sql_budget: |_me, state| {
        // The window opens after the reboot, so only `Reboot`'s bookkeeping is
        // inside it.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 4 + docs_tolerance(state),
        }
    }
}
