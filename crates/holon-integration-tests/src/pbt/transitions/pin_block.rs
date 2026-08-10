//! Transition: pin a block to the right sidebar (LogSeq-style shift+click).
//!
//! @pbt rung gesture
//!   The SUT body drives a real shift+click on the block's bullet: the
//!   modifier-keyed intent lookup resolves the bullet's `shift_action`
//!   wiring and dispatches it. Nothing about `focus_pin` is hardcoded
//!   test-side — op name, destination region and block id all come from
//!   `block_profile.yaml`.
//! @pbt covers pin-sidebar — shift+click a bullet to pin it to the sidebar
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
//! shift-click handler dispatches `focus_pin` in the default layout.
//!
//! KNOWN GAP (known-reds entry 14, measured 2026-08-04): targets come from
//! `main_editable_descendants()`, which has NO depth filter, while the compiled
//! main-panel matview truncates its recursion at nesting depth 20
//! (`WHERE _vl2.depth < 20 … AND _vl2.depth <= 20`; see
//! `crates/holon/tests/turso_storage_repros/tabs_main_panel_delivery.rs:130`).
//! So the generator can shift-click a bullet nested deeper than 20, which the
//! panel never renders and no user could hit. Panel WIDTH is irrelevant — a
//! 40-block flat panel renders all 40 rows and pins the 40th for 17 reads.
//! Such a draw burns the driver's 2s click-resolve poll, dispatches no pin,
//! and reds `inv-focus-roots` / `inv-main-panel-rows-match-focus` / the pinned
//! `PinBlock` read budget at once. The fix is to teach the candidate set the
//! same depth cap the panel query applies.

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
use crate::pbt::transition_budgets::CLICK_JITTER_TOLERANCE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::JOURNAL_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::NAV_DML_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::PIN_BLOCK_CLICK_RESOLVE_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;

/// Pin a block to the right sidebar via shift+click semantics.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I pin block {block_id} in region {region}")]
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
            // edit transitions (NavigateFocus is weight 3). Biased up under
            // `HOLON_PBT_UNDO_REDO_DENSITY=high`: an open pin is the only
            // reference site in the keystone alphabet that SURVIVES an undo
            // (pins push no undo snapshot), so it is the only way a sweep can
            // observe whether a redo heals references — see
            // `crate::pbt::undo_redo_density`.
            (crate::pbt::undo_redo_density::weight(2), strat)
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
    sql_budget: |_me, _state| {
        // focus_pin = SELECT (existence check) + INSERT or UPDATE, on top of the
        // reactive base. Shift+click enters through the rendered bullet, so the
        // click-resolve snapshot is on the path too — hence the PINNED
        // `PIN_BLOCK_CLICK_RESOLVE_READS` ceiling (17 reads), enforced
        // regardless of `HOLON_PERF_BUDGET` (see `sql_reads_pinned`).
        //
        // NO per-watch term, unlike the document-mutating siblings: a pin
        // mutates no block, so no CDC fires and no user watch re-evaluates.
        // Measured 17 reads at watches=0, 1 and 2 alike — see the
        // `watch-bearing-click-nav-sql-budget` hand-authored case, which keeps
        // that regime sampled.
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS + PIN_BLOCK_CLICK_RESOLVE_READS,
            writes: 0,
            ddl: 0,
            tolerance: CLICK_JITTER_TOLERANCE,
        }
    }
}
