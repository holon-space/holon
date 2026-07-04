//! Transition: unpin a block from a sidebar (LogSeq-style X button).
//!
//! Mirrors the `navigation.close(history_id)` op invoked by the right
//! sidebar's per-row X button. Production behavior:
//! - UPDATE `closed_at` on one specific `navigation_history` row.
//! - Cursor is untouched (close removes from the open-pins set, not from the
//!   back/forward stack).
//!
//! Generator picks an open pin that has a non-NULL `block_id` and is NOT its
//! region's current cursor focus. The X button renders on *pins* — open
//! `navigation_history` rows that are not the region's active back/forward
//! focus. The active focus (the cursor target) is closed by navigating away
//! (focus_replace), never by an X button, so closing it via `navigation.close`
//! is a no-op the reference model must not predict. This predicate is
//! layout-independent: it holds whichever region a user's layout pins into
//! (pin regions have no cursor focus → all their pins are closeable; the main
//! panel's single focus row matches `current_focus(region)` → excluded).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefPinsMut;
use holon_pbt_core::capabilities::SutNavHistoryDrive;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::JOURNAL_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// Unpin (close) one open `navigation_history` row by its id.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UnpinBlock {
    pub history_id: i64,
}

impl<R: RefLifecycle + RefPinsMut> TransitionFactory<R> for UnpinBlock {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: pin/unpin dispatch `navigation` ops backed by the
        // Turso-only `NavigationProvider` (registration.rs:267); there is no
        // Loro-native navigation source (see loro_block_query_source.rs:77).
        // Gate it out of {Loro} slices.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate every region's open-pin ids, then let `preconditions` (the
        // single source of truth for unpinnability) narrow to closeable pins: a
        // region's active cursor-focus row is closed via focus_replace, not the X
        // button, so `is_closeable_pin` excludes it.
        let candidates: Vec<i64> = state
            .open_pin_history_ids()
            .into_iter()
            .filter(|history_id| {
                UnpinBlock {
                    history_id: *history_id,
                }
                .preconditions(state)
                .is_good()
            })
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

impl<R: RefLifecycle + RefPinsMut> TransitionRef<R> for UnpinBlock {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.is_closeable_pin(self.history_id),
                Reason::NoPinsToRemove,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.close_pin(self.history_id);
    }
}

crate::cap_transition! {
    UnpinBlock: SutNavHistoryDrive,
    where R: [ RefLifecycle + RefPinsMut ],
    |me, _state, sut| {
        sut.unpin_block(me.history_id).await;
    }
    sql_budget: |_me, state| {
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
