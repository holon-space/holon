//! Transition: create a stale/corrupted .loro file before app startup.
//!
//! Mirrors the legacy logic split across `state_machine.rs:371-392` (generator),
//! `state_machine.rs:3105-3108` (precondition),
//! `state_machine.rs:1942-1946` (ref-state apply),
//! `sut.rs:702-714` (SUT apply), and
//! `transition_budgets.rs:116-125` (expected SQL).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::LoroCorruptionType;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Create a stale/corrupted .loro file BEFORE the system starts.
#[derive(Clone, Debug)]
pub struct CreateStaleLoro {
    /// The org filename this .loro file corresponds to (e.g., "test.org")
    pub org_filename: String,
    /// Type of corruption to simulate
    pub corruption_type: LoroCorruptionType,
}

impl E2ETransitionFactory for CreateStaleLoro {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let early_checks: Vec<Validated<(), Reason>> = vec![
            check(!state.app_started, Reason::AppAlreadyStarted),
            check(state.variant.enable_loro, Reason::LoroDisabledForCorruption),
        ];
        let checks_result = early_checks.into_iter().collect::<Validated<Vec<()>, _>>();
        if checks_result.is_fail() {
            return checks_result.map(|_| unreachable!());
        }

        let org_filenames: Vec<String> = state.documents.values().cloned().collect();
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

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for CreateStaleLoro {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(!state.app_started, Reason::AppAlreadyStarted),
            check(state.variant.enable_loro, Reason::LoroDisabledForCorruption),
            check(
                state.documents.values().any(|f| f == &self.org_filename),
                Reason::NoDocumentsAvailable,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, _: &mut ReferenceState) {
        // CreateStaleLoro doesn't change reference state - the blocks from the
        // corresponding org file should still exist after startup. The system
        // should detect the corrupted .loro file and recover from the .org file.
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_create_stale_loro(&self.org_filename, self.corruption_type)
            .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}
