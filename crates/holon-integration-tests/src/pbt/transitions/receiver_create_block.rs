//! Transition: the RECEIVER instance authors a block — the second writer.
//!
//! @pbt kind transition
//! @pbt covers two-instance-two-writer — a peer-authored block under an
//!   owner-authored parent, and its arrival on the owner after a reverse round.
//!
//! ## What the model predicts, and what it refuses to predict
//! The owner-side [`crate::pbt::reference_state::ReferenceState`] models ONE
//! writer, and widening it to two would mean re-implementing RGA tiebreaks and
//! loro tree-move resolution in the model — the very thing the convergence
//! oracle exists to avoid. So a peer write does NOT enter the owner's block
//! tree. It is recorded in the sharing overlay as `(id, parent)`: membership
//! and parent identity, both order-independent. Sibling order among the peer's
//! blocks is left to the convergence law.
//!
//! ## Why the id is born-equal
//! `SutTwoInstance::peer_create_block` dispatches the receiver's production
//! create with this exact id, so the block the model names IS the block the
//! receiver mints. The owner-side synthetic→real reconcile never sees it: on
//! the owner it arrives by sync, and the harness classifies it as foreign by
//! CRDT authorship (`ComposedSlice::foreign_ids`).
//!
//! ## Why the parent must already be delivered
//! The receiver can only create under a block it holds. The model's
//! `blocks_delivered_to_receiver` is what the last owner→receiver round could
//! carry, so parenting under one of those is a claim the model can make without
//! observing the SUT.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::PeerWrite;
use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::RefSharedViewMut;
use holon_pbt_core::capabilities::SutTwoInstance;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Scheme prefix of a peer-authored block id. Deliberately NOT `block:peer-`:
/// that scheme is the keystone's simulated-peer merge, which the composed
/// reconcile excuses by a hardcoded predicate. A peer write must be excused by
/// AUTHORSHIP instead, so reusing that prefix would hide the seam under test.
pub const PEER_WRITE_PREFIX: &str = "pair-";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I create a block {content} with id {id} on the receiver under {parent}")]
pub struct ReceiverCreateBlock {
    pub parent: EntityUri,
    pub content: String,
    /// Born-equal: the receiver's production create receives this id verbatim.
    pub id: EntityUri,
}

/// Parents the receiver provably holds, in a stable order.
fn deliverable_parents<R: RefSharedView>(state: &R) -> Vec<EntityUri> {
    state.blocks_delivered_to_receiver().into_iter().collect()
}

impl<R: RefSharedView + RefSharedViewMut> TransitionFactory<R> for ReceiverCreateBlock {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let parents = deliverable_parents(state);
        check(
            state.is_shared()
                && !parents.is_empty()
                && crate::pbt::sharing_state::two_writer_alphabet_enabled(),
            Reason::PreconditionFailed,
        )
        .map(|()| {
            // Deterministic and collision-free: the peer-write count is the
            // sequence number, so a replayed draw mints the same id.
            let next = state.peer_writes_pending().len() + state.peer_writes_delivered().len();
            let id = EntityUri::block(&format!("{PEER_WRITE_PREFIX}{next}"));
            let strat = (
                proptest::sample::select(parents),
                proptest::string::string_regex("[a-z]{1,8}").expect("valid regex"),
            )
                .prop_map(move |(parent, content)| ReceiverCreateBlock {
                    parent,
                    content,
                    id: id.clone(),
                })
                .boxed();
            // Moderate: the second writer is the point of this slice, but a
            // sequence made only of peer writes never syncs them anywhere.
            (20, strat)
        })
    }
}

impl<R: RefSharedView + RefSharedViewMut> TransitionRef<R> for ReceiverCreateBlock {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.is_shared(), Reason::PreconditionFailed),
            check(
                state.blocks_delivered_to_receiver().contains(&self.parent),
                Reason::PreconditionFailed,
            ),
            check(!self.content.is_empty(), Reason::PreconditionFailed),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.note_peer_write(
            self.id.clone(),
            PeerWrite {
                parent: self.parent.clone(),
                content: self.content.clone(),
            },
        );
    }
}

crate::cap_transition! {
    ReceiverCreateBlock: SutTwoInstance,
    where R: [ RefSharedView + RefSharedViewMut ],
    |me, _state, sut| {
        sut.peer_create_block(&me.parent, &me.content, &me.id).await;
    }
    sql_budget: |_me, _state| {
        // The write lands in the RECEIVER's engine, which the owner-side span
        // collector does not trace.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 8,
        }
    }
}
