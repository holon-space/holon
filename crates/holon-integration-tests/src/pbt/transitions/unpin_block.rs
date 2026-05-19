//! Transition: unpin a block from a sidebar (LogSeq-style X button).
//!
//! Mirrors the `navigation.close(history_id)` op invoked by the right
//! sidebar's per-row X button. Production behavior:
//! - UPDATE `closed_at` on one specific `navigation_history` row.
//! - Cursor is untouched (close removes from the open-pins set, not from
//!   the back/forward stack).
//!
//! Generator picks any open pin from `state.open_pins`; precondition
//! requires at least one open pin row with a non-NULL `block_id` (home
//! rows are skipped — the X button only renders on pinned blocks).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, JOURNAL_READS, REACTIVE_BASE, docs_tolerance};

/// Unpin (close) one open `navigation_history` row by its id.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UnpinBlock {
    pub history_id: i64,
}

impl E2ETransitionFactory for UnpinBlock {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (all open pins) and let `preconditions`
        // be the single source of truth for which ones are actually unpinnable.
        // Avoids duplicating the non-NULL block_id check across two sites.
        let candidates: Vec<i64> = state
            .open_pins
            .values()
            .flat_map(|pins| pins.iter())
            .filter(|p| {
                UnpinBlock {
                    history_id: p.history_id,
                }
                .preconditions(state)
                .is_good()
            })
            .map(|p| p.history_id)
            .collect();
        check(!candidates.is_empty(), Reason::NoPinsToRemove).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|history_id| UnpinBlock { history_id })
                .boxed();
            // Weight 2 — symmetric with PinBlock.
            (2, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for UnpinBlock {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(
                state.open_pins.values().any(|pins| {
                    pins.iter()
                        .any(|p| p.history_id == self.history_id && p.block_id.is_some())
                }),
                Reason::NoPinsToRemove,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        for pins in state.open_pins.values_mut() {
            pins.retain(|p| p.history_id != self.history_id);
        }
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_unpin_block(self.history_id).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        // close = single UPDATE statement; reactive watchers re-run on the CDC
        // delta. No SELECT round-trip needed (the X button supplied the id).
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}
