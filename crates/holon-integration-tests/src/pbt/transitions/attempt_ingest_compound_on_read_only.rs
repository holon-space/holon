//! Transition: aim an INGEST-origin compound at a block homed in a
//! `WriteTier::ReadOnly` file.
//!
//! @pbt rung storage
//! @pbt covers compound-constituent-origin — a compound's constituents must be
//!   judged under the COMPOUND's provenance, not re-judged as a user's edit
//!
//! Sibling of [`AttemptReadOnlyEdit`](super::AttemptReadOnlyEdit), and the
//! other half of the same gate. That one proves a USER's write is refused;
//! this one proves the refusal does not spread to origins the gate exempts.
//!
//! A compound decomposes into constituent writes the engine sends straight to
//! the dispatcher. Dispatching them as a user's edit makes an ingest that
//! rewrites a read-only-homed block's source fail — the file telling the store
//! what it says, refused on the grounds that the store may not tell the file.
//!
//! The compound writes the block's own stored source back, so an accepted one
//! leaves `block_raw` exactly as the ingest left it and every block invariant
//! keeps comparing against the same bytes. The oracle applies it as a NO-OP
//! for that reason.

use holon_pbt_core::RequiredWiring;
use holon_pbt_core::StorageAdapter;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefReadOnlyHomes;
use holon_pbt_core::capabilities::SutReadOnlyEditAttempt;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Re-write the read-only-homed block `block_id`'s own source under an
/// ingest-origin compound.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("An ingest re-writes the source of the read-only-homed block {block_id}")]
pub struct AttemptIngestCompoundOnReadOnly {
    pub block_id: String,
}

impl TransitionFactory<ReferenceState> for AttemptIngestCompoundOnReadOnly {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> RequiredWiring {
        // Same arm as `AttemptReadOnlyEdit`: the fixture that seeds the
        // read-only document only exists on a Turso draw.
        RequiredWiring::HasStorage(StorageAdapter::Turso)
    }

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let targets: Vec<String> = state
            .read_only
            .homes()
            .iter()
            .map(|id| id.to_string())
            .collect();
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!targets.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            let strat = proptest::sample::select(targets)
                .prop_map(|block_id| AttemptIngestCompoundOnReadOnly { block_id })
                .boxed();
            // Same low weight as its sibling, and for the same reason: one
            // exempt compound per run arms the clause.
            (3, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for AttemptIngestCompoundOnReadOnly {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state
                    .read_only
                    .homes()
                    .iter()
                    .any(|id| id.as_str() == self.block_id),
                Reason::PreconditionFailed,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, _: &mut ReferenceState) {
        // The compound writes the block's own source back, so the store is
        // unchanged whether it is accepted or refused. The judgment lives in
        // `inv-read-only-home-refuses-writes`.
    }
}

crate::cap_transition! {
    AttemptIngestCompoundOnReadOnly: SutReadOnlyEditAttempt,
    where R: [ RefReadOnlyHomes ],
    |me, _state, sut| {
        // Acceptance is the expected outcome and is judged by the invariant;
        // a refusal must reach it, not die in an assert here.
        let _ = sut.attempt_ingest_compound(&me.block_id).await;
    }
    sql_budget: |_me, _state| {
        // The compound reads the block's source, its owning document's
        // vocabulary and its stored keyword, then writes the same content back
        // through one constituent — which the store recognises as a no-op, so
        // no row is written.
        ExpectedSql {
            reads: 7,
            writes: 0,
            ddl: 0,
            tolerance: 32,
        }
    }
}
