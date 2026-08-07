//! Transition: cmd+click a left-sidebar row to open it as an additional tab.
//!
//! @pbt rung gesture
//!   The SUT body drives a real cmd+click on the sidebar row; the
//! modifier-keyed   intent lookup resolves the row's `cmd_action` wiring and
//! dispatches it.   Op name, region and block id all come from
//! `assets/default/index.org` —   nothing about `open_tab` is hardcoded
//! test-side. @pbt covers open-in-tab — cmd+click a sidebar page to open a
//! background tab
//!
//! Production semantics (`crates/holon/src/navigation/provider.rs::open_tab`):
//! an open row for `(region, block_id)` → `activate` it (cursor moves, NO new
//! row); otherwise INSERT a new open row WITHOUT closing the region's others.
//! Either way the cursor ends on the clicked row, so the panel shows it while
//! the previously-open rows stay open but unrendered — see
//! `ReferenceState::rendered_focus_root`.
//!
//! The sidebar declares `cmd_action` (macOS) and `ctrl_action` (Windows/Linux)
//! as the same op, so `use_ctrl` picks which modifier carries the gesture and
//! the generator draws both — keeping that equivalence exercised rather than
//! assumed.

use holon_api::EntityUri;
use holon_api::Region;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefNavHistory;
use holon_pbt_core::capabilities::RefNavHistoryMut;
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
use crate::pbt::transition_budgets::OPEN_TAB_ACTIVATE_CLICK_RESOLVE_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::OPEN_TAB_INSERT_CLICK_RESOLVE_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;

/// Open a sidebar page as an additional Main tab via cmd- or ctrl-click.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct OpenTabViaModifierClick {
    pub block_id: EntityUri,
    /// Which modifier carries the gesture. The sidebar declares BOTH
    /// `cmd_action` (macOS) and `ctrl_action` (Windows/Linux) pointing at the
    /// same `navigation_open_tab`, so the two are interchangeable by
    /// construction — generating both keeps that equivalence honest instead of
    /// asserting it once. Defaults to cmd so hand-authored cases may omit it.
    #[serde(default)]
    pub use_ctrl: bool,
}

impl<R: RefLifecycle + RefBlockTree + RefNavHistory + RefNavHistoryMut> TransitionFactory<R>
    for OpenTabViaModifierClick
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only for the same reason as `PinBlock`: `open_tab` is a
        // `navigation` provider op and the provider is Turso-backed.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Exactly the rows production wraps in the sidebar `item_template` —
        // the same set `ClickBlock`'s LeftSidebar arm draws from, so every
        // candidate provably carries the `cmd_action` wiring.
        let candidates: Vec<EntityUri> = state.predicted_sidebar_navigation_targets();
        check(!candidates.is_empty(), Reason::NoFocusableBlocks).map(|_| {
            let strat = (prop::sample::select(candidates), any::<bool>())
                .prop_map(|(block_id, use_ctrl)| OpenTabViaModifierClick { block_id, use_ctrl })
                .boxed();
            // Weight 2, matching PinBlock: often enough to grow the open set
            // (and to revisit an already-open row, exercising the activate
            // branch) without crowding out navigation and editing.
            (2, strat)
        })
    }
}

impl<R: RefLifecycle + RefBlockTree + RefNavHistory + RefNavHistoryMut> TransitionRef<R>
    for OpenTabViaModifierClick
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state
                    .predicted_sidebar_navigation_targets()
                    .contains(&self.block_id),
                Reason::PreconditionFailed,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.nav_open_tab(Region::Main, &self.block_id);
    }
}

crate::cap_transition! {
    OpenTabViaModifierClick: SutNavHistoryDrive,
    where R: [ RefLifecycle + RefBlockTree + RefNavHistory + RefNavHistoryMut ],
    |me, _state, sut| {
        sut.open_tab_via_modifier_click(&me.block_id, me.use_ctrl).await;
    }
    sql_budget: |_me, state| {
        // `open_tab` has TWO branches and they issue different SQL, so each
        // carries its own PINNED ceiling: activate (row already open —
        // cursor-only) costs one read less than insert (append a new row, which
        // also reads `MAX(id)`). Which branch ran comes from the reference's
        // `last_open_tab_activated`, recorded at apply time because the
        // post-apply state has the row open under either branch.
        //
        // Enforced regardless of `HOLON_PERF_BUDGET` (see `sql_reads_pinned`).
        //
        // TOLERANCE IS ZERO, and it has to be: the branch delta is exactly 1,
        // so ANY tolerance makes the activate ceiling reach the insert cost and
        // the two pins overlap — a backwards flag would pass both branches and
        // this whole split would assert nothing. Zero is what the measurements
        // support: every sample of both branches lands EXACTLY on its pin. If a
        // real render-coalescing jitter ever shows up here it must red, be
        // measured, and be modelled explicitly the way the branch now is —
        // never re-absorbed into a blanket pad that hides the branch again.
        //
        // NO first-visit term, unlike `NavigateFocus`, and no per-watch term,
        // unlike the document-mutating siblings — both were measured at zero
        // here. `open_tab` appends a row instead of closing the region's others,
        // so the panel keeps rendering the same subtree and no watch matview is
        // created: first-visit opens cost the same reads and 0 DDL as revisits,
        // and watches cost nothing (no block mutation ⇒ no CDC).
        // Kept sampled by the hand-authored cases (first visit) and
        // `watch-bearing-click-nav-sql-budget` (watches=2).
        let click_resolve = if state.last_open_tab_activated() {
            OPEN_TAB_ACTIVATE_CLICK_RESOLVE_READS
        } else {
            OPEN_TAB_INSERT_CLICK_RESOLVE_READS
        };
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS + click_resolve,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}
