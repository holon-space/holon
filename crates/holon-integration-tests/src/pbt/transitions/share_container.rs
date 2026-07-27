//! Transition: share the whole vault (root container) with the receiver.
//!
//! @pbt kind transition
//! @pbt covers two-instance-share — ShareContainer commits the owner's policy +
//!   membership cert and puts the root container into the replication set.
//!
//! **Whole vault = the replication set, not a mega container** (true-sharing
//! plan §B): sharing the root container id makes every container in
//! `ContainerRegistry::replication_set` eligible, with per-container keys and
//! epochs intact. Per-container selectors are the same transition with a
//! different selector.
//!
//! **H3 ordering.** The reference widens the POLICY audience first (as a
//! whole-vault default, so blocks created after the share are covered too), and
//! only then is anything observable in the shared container. That keeps
//! `inv-audience-never-over-approximates` green by construction: the effective
//! audience can never exceed a policy that already covers every block.
//!
//! Cap-gated on `SutTwoInstance`, which only the two-instance slice provides —
//! every single-instance draw excludes this variant from its alphabet.

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
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

use crate::pbt::sharing_state::RECEIVER_PRINCIPAL;

/// The container selector a whole-vault share names — the root container id the
/// registry advertises the global doc under
/// (`holon_loro::container_registry::ROOT_CONTAINER_ID`).
pub const ROOT_SELECTOR: &str = "holon_tree";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ShareContainer {
    pub selector: String,
    pub principal: String,
}

impl<R: RefSharedView + RefSharedViewMut> TransitionFactory<R> for ShareContainer {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let me = ShareContainer {
            selector: ROOT_SELECTOR.to_string(),
            principal: RECEIVER_PRINCIPAL.to_string(),
        };
        me.preconditions(state).map(|()| {
            // Heavy: a sequence that never shares can only ever exercise the
            // negative (nothing-crosses) branch, and the share is a once-per-run
            // event, so it must be reached early to leave ticks for a SyncNow.
            (30, Just(me).boxed())
        })
    }
}

impl<R: RefSharedView + RefSharedViewMut> TransitionRef<R> for ShareContainer {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        // Once only: re-sharing an already-shared vault is a no-op that would
        // just burn ticks the sequence needs for syncing.
        check(!state.is_shared(), Reason::VaultAlreadyShared)
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.apply_share_vault(&self.principal);
    }
}


crate::cap_transition! {
    ShareContainer: SutTwoInstance,
    where R: [ RefSharedView + RefSharedViewMut ],
    |me, _state, sut| {
        sut.share_container(&me.selector, &me.principal).await;
    }
    sql_budget: |_me, _state| {
        // Policy commit + cert issue live in holon-sharing's in-memory objects; nothing touches the traced engine.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 4,
        }
    }
}

