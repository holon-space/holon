//! Transition: unpin a block from a sidebar (LogSeq-style X button).
//!
//! Mirrors the `navigation.close(history_id)` op invoked by the right
//! sidebar's per-row X button. Production behavior:
//! - UPDATE `closed_at` on one specific `navigation_history` row.
//! - Cursor is untouched (close removes from the open-pins set, not from
//!   the back/forward stack).
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

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::capabilities::SutNavHistoryDrive;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, JOURNAL_READS, REACTIVE_BASE, docs_tolerance};

/// Unpin (close) one open `navigation_history` row by its id.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UnpinBlock {
    pub history_id: i64,
}

impl TransitionFactory<ReferenceState> for UnpinBlock {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutNavHistoryDrive,
        >()]
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: pin/unpin dispatch `navigation` ops backed by the
        // Turso-only `NavigationProvider` (registration.rs:267); there is no
        // Loro-native navigation source (see loro_block_query_source.rs:77).
        // Gate it out of {Loro} slices.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate every region's open pins, excluding each region's current
        // cursor focus (handled by `preconditions`, the single source of truth
        // for unpinnability). A region's active focus row is the cursor target
        // — closed via focus_replace, not the X button.
        let candidates: Vec<i64> = state
            .ui
            .user
            .open_pins
            .values()
            .flatten()
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

impl TransitionRef<ReferenceState> for UnpinBlock {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
            check(
                state.ui.user.open_pins.iter().any(|(region, pins)| {
                    let focus = state.current_focus(*region);
                    pins.iter().any(|p| {
                        p.history_id == self.history_id
                            && p.block_id.is_some()
                            && p.block_id != focus
                    })
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
        for pins in state.ui.user.open_pins.values_mut() {
            pins.retain(|p| p.history_id != self.history_id);
        }
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutNavHistoryDrive> TransitionImpl<ReferenceState, S> for UnpinBlock {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.unpin_block(self.history_id).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for UnpinBlock {
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
