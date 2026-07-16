//! Transition: create a stale/corrupted .loro file before app startup.
//!
//! @pbt rung external
//!   writes a corrupt .loro file before boot (fs stimulus).
//! @pbt covers stale-loro-recovery — corrupt CRDT file at startup
//!
//! Mirrors the legacy logic split across `state_machine.rs:371-392`
//! (generator), `state_machine.rs:3105-3108` (precondition),
//! `state_machine.rs:1942-1946` (ref-state apply),
//! `sut.rs:702-714` (SUT apply), and
//! `transition_budgets.rs:116-125` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
#[cfg(feature = "otel-testing")]
use holon_pbt_core::budget::ExpectedSql;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::types::LoroCorruptionType;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

/// Create a stale/corrupted .loro file BEFORE the system starts.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CreateStaleLoro {
    /// The org filename this .loro file corresponds to (e.g., "test.org")
    pub org_filename: String,
    /// Type of corruption to simulate
    pub corruption_type: LoroCorruptionType,
}

impl<R: RefLifecycle + RefDocuments> TransitionFactory<R> for CreateStaleLoro {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Loro)
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let early_checks: Vec<Validated<(), Reason>> = vec![
            check(!state.app_started(), Reason::AppAlreadyStarted),
            check(state.enable_loro(), Reason::LoroDisabledForCorruption),
        ];
        let checks_result = early_checks.into_iter().collect::<Validated<Vec<()>, _>>();
        if checks_result.is_fail() {
            return checks_result.map(|_| unreachable!());
        }

        let org_filenames: Vec<String> = state.document_names();
        if org_filenames.is_empty() {
            return Validated::fail(Reason::NoDocumentsAvailable);
        }

        let corruption_types = [
            LoroCorruptionType::Empty,
            LoroCorruptionType::Truncated,
            LoroCorruptionType::InvalidHeader,
        ];

        let candidates: Vec<(String, LoroCorruptionType)> = org_filenames
            .iter()
            .flat_map(|org_filename| {
                corruption_types.iter().filter_map(move |&corruption_type| {
                    let instance = CreateStaleLoro {
                        org_filename: org_filename.clone(),
                        corruption_type,
                    };
                    if instance.preconditions(state).is_good() {
                        Some((org_filename.clone(), corruption_type))
                    } else {
                        None
                    }
                })
            })
            .collect();

        check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|(org_filename, corruption_type)| CreateStaleLoro {
                    org_filename,
                    corruption_type,
                })
                .boxed();
            (1, strat)
        })
    }
}

impl<R: RefLifecycle + RefDocuments> TransitionRef<R> for CreateStaleLoro {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(!state.app_started(), Reason::AppAlreadyStarted),
            check(state.enable_loro(), Reason::LoroDisabledForCorruption),
            check(
                state.has_document(&self.org_filename),
                Reason::NoDocumentsAvailable,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, _: &mut R) {
        // CreateStaleLoro doesn't change reference state - the blocks from the
        // corresponding org file should still exist after startup. The system
        // should detect the corrupted .loro file and recover from the .org
        // file.
    }
}

holon_pbt_core::cap_transition! {
    CreateStaleLoro: holon_pbt_core::capabilities::SutFixtureFs,
    where R: [ RefLifecycle + RefDocuments ],
    |me, _state, sut| {
        sut.create_stale_loro(&me.org_filename, me.corruption_type)
            .await;
    }
    sql_budget: |_me, _state| {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}
