//! Transition: run ONE bounded sync round between the two instances.
//!
//! @pbt kind transition
//! @pbt covers two-instance-sync — SyncNow drives `sync_once` over the relay in
//!   one direction and records the round in the model.
//!
//! The transport seam has no `subscribe`, so cadence is the caller's: this
//! transition IS the cadence in the PBT, exactly as a timer or foreground hook
//! is in production. Drawable even when nothing is shared — that draw is the
//! Inc0 negative case (`sync_once` runs, consults the transport, and transports
//! nothing), which the convergence invariant's unshared branch asserts.

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::RefSharedViewMut;
use holon_pbt_core::capabilities::SutTwoInstance;
use holon_pbt_core::validation::Reason;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;
use validated::Validated::Good;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I sync now owner-to-receiver {owner_to_receiver}")]
pub struct SyncNow {
    /// `true` = the owner publishes and the receiver admits; `false` = the
    /// reverse. Both directions run the SAME orchestrator over the SAME relay.
    pub owner_to_receiver: bool,
}

impl<R: RefSharedView + RefSharedViewMut> TransitionFactory<R> for SyncNow {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // BOTH directions, with the reverse leg gated on a shared vault and
        // weighted up once a peer write is outstanding: the reverse round
        // imports receiver-authored state, which is a sharing finding only when
        // there is a share and something to carry.
        let reverse_weight = match (
            state.is_shared() && crate::pbt::sharing_state::two_writer_alphabet_enabled(),
            state.peer_writes_pending().is_empty(),
        ) {
            (false, _) => 0,
            (true, true) => 1,
            (true, false) => 4,
        };
        Good((
            25,
            if reverse_weight == 0 {
                Just(SyncNow {
                    owner_to_receiver: true,
                })
                .boxed()
            } else {
                prop_oneof![
                    3 => Just(SyncNow { owner_to_receiver: true }),
                    reverse_weight => Just(SyncNow { owner_to_receiver: false }),
                ]
                .boxed()
            },
        ))
    }
}

impl<R: RefSharedView + RefSharedViewMut> TransitionRef<R> for SyncNow {
    type Reason = Reason;

    fn preconditions(&self, _: &R) -> Validated<(), Reason> {
        Good(())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // A round only counts as a delivery when there is something to deliver:
        // unshared, the acceptor refuses everything, so the receiver's state is
        // unchanged and the model must NOT start expecting convergence.
        if !state.is_shared() {
            return;
        }
        if self.owner_to_receiver {
            state.note_owner_to_receiver_round();
        } else {
            // The receiver→owner leg is what the model requires before it
            // expects a peer write on the owner. A bidirectional wire may
            // deliver earlier — the model states a LOWER bound, so an early
            // delivery is not a violation.
            state.note_receiver_to_owner_round();
        }
    }
}

crate::cap_transition! {
    SyncNow: SutTwoInstance,
    where R: [ RefSharedView + RefSharedViewMut ],
    |me, _state, sut| {
        sut.sync_now(me.owner_to_receiver).await;
    }
    sql_budget: |_me, _state| {
        // A round moves Loro blobs; the receiver's SQL projection is driven by ITS OWN engine, which the owner-side span collector does not trace.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 8,
        }
    }
}
