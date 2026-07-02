//! Transition: pin a block to the right sidebar (LogSeq-style shift+click).
//!
//! Mirrors the `navigation.focus_pin(region, block_id)` op invoked by the
//! GPUI bullet's shift-click action (see `frontends/gpui/src/views/render_entity_view.rs`).
//! Production behavior:
//! - Existing open pin for `(region, block_id)` → UPDATE timestamp (move-to-top).
//! - No existing open pin → INSERT new row.
//! Cursor is untouched (pins are not part of back/forward navigation).
//!
//! Generator restricted to `Region::RightSidebar` — the only place the bullet's
//! shift-click handler dispatches `focus_pin` in the default layout. Targets
//! are `focusable_rendered_block_ids(Region::Main)` (visible blocks in the
//! main panel — the user can only shift-click on a rendered bullet).

use crate::pbt::validation::{Reason, check};
use holon_api::{EntityUri, Region};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::capabilities::{
    RefBlockTree, RefLifecycle, RefNavHistoryMut, SutNavHistoryDrive,
};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

/// Pin a block to the right sidebar via shift+click semantics.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PinBlock {
    pub region: Region,
    pub block_id: EntityUri,
}

impl TransitionFactory<ReferenceState> for PinBlock {
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
        // Candidate set: Main's editable descendants. Per-precondition
        // filter narrows to pinnable subset.
        let candidates: Vec<EntityUri> = state
            .main_editable_descendants()
            .into_iter()
            .filter(|uri| {
                PinBlock {
                    region: Region::RightSidebar,
                    block_id: uri.clone(),
                }
                .preconditions(state)
                .is_good()
            })
            .collect();
        check(!candidates.is_empty(), Reason::NoPinCandidates).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|block_id| PinBlock {
                    region: Region::RightSidebar,
                    block_id,
                })
                .boxed();
            // Weight 2 — pin/unpin should fire often enough to expand the
            // open-pins set, but not drown out the more common navigation +
            // edit transitions (NavigateFocus is weight 3).
            (2, strat)
        })
    }
}

fn pin_block_preconditions<R: RefLifecycle + RefBlockTree>(
    block_id: &EntityUri,
    state: &R,
) -> Validated<(), Reason> {
    let exists = state.block_exists(block_id);
    let mut checks: Vec<Validated<(), Reason>> = vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(exists, Reason::FocusedBlockMissing),
    ];
    if exists {
        checks.push(check(state.is_text_block(block_id), Reason::FocusedNotText));
        checks.push(check(
            !state.is_page_block(block_id),
            Reason::PreconditionFailed,
        ));
    }
    checks.push(check(
        !state.is_layout_block(block_id),
        Reason::FocusedInLayoutBlocks,
    ));
    checks.push(check(
        state.is_focusable(block_id),
        Reason::FocusedNotFocusable,
    ));

    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

fn pin_block_apply_to_ref<R: RefNavHistoryMut>(region: Region, block_id: EntityUri, state: &mut R) {
    // Move-to-top dedup, mirroring `provider.rs::focus_pin`:
    // SELECT existing open `(region, block_id)`; UPDATE timestamp if
    // found, else INSERT. Bumping `next_pin_ts` (not `next_history_id`)
    // matches the no-INSERT path of `update_pin_timestamp.sql`.
    state.add_pin(region, block_id);
}

impl TransitionRef<ReferenceState> for PinBlock {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        pin_block_preconditions(&self.block_id, state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        pin_block_apply_to_ref(self.region, self.block_id.clone(), state);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutNavHistoryDrive> TransitionImpl<ReferenceState, S> for PinBlock {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.pin_block(self.region, &self.block_id).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for PinBlock {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        // focus_pin = SELECT (existence check) + INSERT or UPDATE.
        // Two round-trips total — one read and one write. The reactive base
        // captures the watcher activity; NAV_DML_READS covers the SELECT.
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}
