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

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::Region;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::SutNavHistoryDrive;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::OpenPinEntry;
use crate::pbt::reference_state::ReferenceState;
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
use crate::pbt::validation::Reason;
use crate::pbt::validation::check;

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

impl TransitionRef<ReferenceState> for PinBlock {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let block = state.domain.block_state.blocks.get(&self.block_id);
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
            check(block.is_some(), Reason::FocusedBlockMissing),
        ];
        if let Some(b) = block {
            checks.push(check(
                b.content_type == ContentType::Text,
                Reason::FocusedNotText,
            ));
            checks.push(check(!b.is_page(), Reason::PreconditionFailed));
        }
        checks.push(check(
            !state.domain.layout_blocks.contains(&self.block_id),
            Reason::FocusedInLayoutBlocks,
        ));
        checks.push(check(
            state.domain.layout_blocks.is_focusable(&self.block_id),
            Reason::FocusedNotFocusable,
        ));

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        // Move-to-top dedup, mirroring `provider.rs::focus_pin`:
        // SELECT existing open `(region, block_id)`; UPDATE timestamp if
        // found, else INSERT. Bumping `next_pin_ts` (not `next_history_id`)
        // matches the no-INSERT path of `update_pin_timestamp.sql`.
        let added_ts_logical = state.ui.user.next_pin_ts;
        state.ui.user.next_pin_ts += 1;

        let pins = state.ui.user.open_pins.entry(self.region).or_default();
        if let Some(existing) = pins
            .iter_mut()
            .find(|p| p.block_id.as_ref() == Some(&self.block_id))
        {
            existing.added_ts_logical = added_ts_logical;
        } else {
            let history_id = state.ui.tab.next_history_id;
            state.ui.tab.next_history_id += 1;
            state
                .ui
                .user
                .open_pins
                .entry(self.region)
                .or_default()
                .push(OpenPinEntry {
                    history_id,
                    block_id: Some(self.block_id.clone()),
                    added_ts_logical,
                });
        }
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
