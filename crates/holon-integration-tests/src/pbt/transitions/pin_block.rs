//! Transition: pin a block to the right sidebar (LogSeq-style shift+click).
//!
//! Mirrors the `navigation.focus_pin(region, block_id)` op invoked by the
//! GPUI bullet's shift-click action (see
//! `frontends/gpui/src/views/render_entity_view.rs`). Production behavior:
//! - Existing open pin for `(region, block_id)` → UPDATE timestamp
//!   (move-to-top).
//! - No existing open pin → INSERT new row.
//! Cursor is untouched (pins are not part of back/forward navigation).
//!
//! Generator restricted to `Region::RightSidebar` — the only place the bullet's
//! shift-click handler dispatches `focus_pin` in the default layout. Targets
//! are `focusable_rendered_block_ids(Region::Main)` (visible blocks in the
//! main panel — the user can only shift-click on a rendered bullet).

use holon_api::EntityUri;
use holon_api::Region;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
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
use crate::pbt::transition_budgets::NAV_DML_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// Pin a block to the right sidebar via shift+click semantics.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PinBlock {
    pub region: Region,
    pub block_id: EntityUri,
}

impl<R: RefLifecycle + RefBlockTree + RefPinsMut> TransitionFactory<R> for PinBlock {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        // Single-sourced from the `cap_transition!` below — cannot drift with the
        // `S: SutNavHistoryDrive` dispatch bound (both come from the one cap token).
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

impl<R: RefLifecycle + RefBlockTree + RefPinsMut> TransitionRef<R> for PinBlock {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let block_exists = state.block_content(&self.block_id).is_some();
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(block_exists, Reason::FocusedBlockMissing),
        ];
        if block_exists {
            checks.push(check(
                state.is_text_block(&self.block_id),
                Reason::FocusedNotText,
            ));
            checks.push(check(
                !state.is_page_block(&self.block_id),
                Reason::PreconditionFailed,
            ));
        }
        checks.push(check(
            !state.is_layout_block(&self.block_id),
            Reason::FocusedInLayoutBlocks,
        ));
        checks.push(check(
            state.is_focusable(&self.block_id),
            Reason::FocusedNotFocusable,
        ));

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // Move-to-top dedup + counter bookkeeping is encapsulated in the ref cap
        // (mirrors `provider.rs::focus_pin`); the transition just declares intent.
        state.upsert_open_pin(self.region, &self.block_id);
    }
}

crate::cap_transition! {
    PinBlock: SutNavHistoryDrive,
    where R: [ RefLifecycle + RefBlockTree + RefPinsMut ],
    |me, _state, sut| {
        sut.pin_block(me.region, &me.block_id).await;
    }
    sql_budget: |_me, state| {
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
