//! Transition: mutate the fixture peer's list and run ONE remote-list sync
//! round, then run a second round to prove the converged round is a no-op.
//!
//! @pbt rung remote-list
//! @pbt covers remote-list-mirror-matches-ref — the round drives the REAL
//!   [`holon_connections::sync_once`] over a fixture peer and the real mirror
//!   table; the invariant then asserts the mirror equals the peer's list.
//!
//! The peer is the oracle. The SUT half mutates the peer the same way the ref
//! half does, runs the production round (pull → reconcile → apply local
//! intents through the dispatcher → push), and then runs a SECOND round over
//! the unchanged peer and fails loud unless it committed nothing — the
//! idempotence a sync round must have.

use holon_pbt_core::RequiredWiring;
use holon_pbt_core::StorageAdapter;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RemoteListMutation;
use holon_pbt_core::capabilities::SutRemoteListSync;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use proptest::strategy::Union;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Mutate the fixture peer's list and run one sync round, then a second round
/// that must be a no-op.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I mutate the remote list and sync one round {mutation}")]
pub struct RemoteListSync {
    pub mutation: RemoteListMutation,
}

impl TransitionFactory<ReferenceState> for RemoteListSync {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> RequiredWiring {
        // The round applies its local writes through the production dispatcher
        // into the persisted mirror table — pure Turso.
        RequiredWiring::HasStorage(StorageAdapter::Turso)
    }

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        vec![check(state.app_started(), Reason::AppNotStarted)]
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(move |_| {
                let next = state.remote_list.next_label();
                let existing = state.remote_list.existing_columns();
                let add = (
                    prop::sample::select(vec!["1", "2", "3"]),
                    prop::sample::select(vec!["0", "1"]),
                )
                    .prop_map(move |(rank, done)| RemoteListMutation::Add {
                        columns: vec![
                            ("label".to_string(), format!("label{next}")),
                            ("bucket".to_string(), "bucket".to_string()),
                            ("rank".to_string(), rank.to_string()),
                            ("done".to_string(), done.to_string()),
                        ],
                    });
                let mut arms: Vec<(u32, BoxedStrategy<RemoteListMutation>)> =
                    vec![(4, add.boxed())];
                if !existing.is_empty() {
                    arms.push((
                        3,
                        prop::sample::select(existing)
                            .prop_map(|columns| RemoteListMutation::Remove { columns })
                            .boxed(),
                    ));
                }
                (
                    8,
                    Union::new_weighted(arms)
                        .prop_map(|mutation| RemoteListSync { mutation })
                        .boxed(),
                )
            })
    }
}

impl TransitionRef<ReferenceState> for RemoteListSync {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![check(state.app_started(), Reason::AppNotStarted)]
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.remote_list.apply(&self.mutation);
    }
}

crate::cap_transition! {
    RemoteListSync: SutRemoteListSync,
    where R: [RefLifecycle],
    |me, _state, sut| {
        sut.remote_list_mutate(me.mutation.clone()).await;
        sut.remote_list_sync_round().await;
        // Idempotence: over an unchanged peer, a second round must decide
        // nothing to push. A round that re-pushes would re-send the same
        // command every poll.
        let second = sut.remote_list_sync_round().await;
        assert!(
            second.committed == 0,
            "remote-list sync: a second round over an unchanged peer committed {} command(s); \
             the round did not converge",
            second.committed
        );
    }
    sql_budget: |_me, _state| {
        // Two rounds, each reading the mirror once; each round's local intents
        // land through the production dispatcher. The peer is in-process, so
        // its own reads never reach the span collector.
        ExpectedSql {
            reads: 2,
            writes: 8,
            ddl: 0,
            tolerance: 64,
        }
    }
}
