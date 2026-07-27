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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

    fn weighted_generator(_: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // ONE-WAY only. A receiver→owner round imports receiver-local state into
        // the owner, which the owner-side oracle does not model — it would
        // surface as an unreconcilable id rather than as a sharing finding. The
        // reverse direction lands with the concurrent-edit increment that gives
        // the model a second side.
        Good((
            25,
            Just(SyncNow {
                owner_to_receiver: true,
            })
            .boxed(),
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
        if self.owner_to_receiver && state.is_shared() {
            state.note_owner_to_receiver_round();
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

