//! Transition: advance the ambient calendar day (ADR 0024 §6 capstone).
//!
//! The Phase-1 rule-firing keystone transition. Drives the day-rollover a real
//! user experiences by advancing the **injected fake `Clock`** and letting the
//! production `ClockScheduler` propagate the new day through the reactive path —
//! the clock-relation UPDATE's CDC re-fires the journal trigger matview, the
//! `action_watcher` fires `block.create` with a WP2 deterministic id, and one
//! journal block per day lands under `block:journals`. The SUT cap NEVER writes
//! the `clock` relation directly (WP1's clock-injection race); it advances the
//! `TestClock` the scheduler already reads and re-runs the scheduler's own
//! `reconcile_clock`.
//!
//! Oracle: `apply_to_ref` advances the reference-model day (`RefClockMut`), which
//! records the distinct day visited. The composed `inv-journal-one-per-day`
//! invariant then asserts the projection holds exactly one journal block per
//! distinct day the clock has visited — so N advances spanning D distinct days ⇒
//! D journal blocks, and a same-day re-tick (`days == 0`) adds nothing (the
//! deterministic id converges to an upsert). Settle is automatic
//! (`S::settle_after_apply` → `converge_projections`), so the reactive rule
//! firing projects through before the invariant runs.

use holon_pbt_core::capabilities::{RefClock, RefClockMut, RefLifecycle};
use holon_pbt_core::validation::{Reason, check};
use holon_pbt_core::{TransitionFactory, TransitionRef};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    CACHE_EVENT_READS, ExpectedSql, JOURNAL_READS, REACTIVE_BASE,
};

/// Advance the ambient calendar day by `days` (`0` = same-day re-tick, the
/// idempotence probe; `1` = day rollover). The generator weights toward `1`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AdvanceDay {
    pub days: i64,
}

impl<R: RefLifecycle + RefClock> TransitionFactory<R> for AdvanceDay {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        check(state.app_started(), Reason::AppNotStarted).map(|_| {
            // Weight rollover 3:1 over the same-day re-tick, so a run mostly grows
            // the journal set (the primary property) while still exercising
            // idempotence. Cap the jump at one day so each rollover fires exactly
            // one new create — the one-per-distinct-day oracle stays legible.
            let strat = prop_oneof![3 => Just(1i64), 1 => Just(0i64)]
                .prop_map(|days| AdvanceDay { days })
                .boxed();
            (10, strat)
        })
    }
}

impl<R: RefLifecycle + RefClock + RefClockMut> TransitionRef<R> for AdvanceDay {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        check(state.app_started(), Reason::AppNotStarted)
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.advance_day(self.days);
    }
}

crate::cap_transition! {
    AdvanceDay: holon_pbt_core::capabilities::SutClockAdvance,
    where R: [ RefLifecycle + RefClock + RefClockMut ],
    |me, _state, sut| {
        sut.advance_clock_days(me.days).await;
    }
    sql_budget: |_me, state| {
        // A rollover is one `clock` UPDATE whose CDC re-fires the journal trigger
        // matview and a single reactive `block.create`; a same-day re-tick is the
        // UPDATE alone (Unchanged, no create). Bound generously — the reactive
        // cascade's read count scales with the block table.
        let blocks = state.block_count();
        ExpectedSql {
            reads: JOURNAL_READS + blocks + CACHE_EVENT_READS,
            writes: 2,
            ddl: 0,
            tolerance: REACTIVE_BASE + blocks,
        }
    }
}
