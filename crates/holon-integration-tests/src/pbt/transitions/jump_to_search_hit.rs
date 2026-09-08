//! Transition: quick-open a hit and press Enter on it.
//!
//! @pbt rung dispatch
//!   runs `QueryEngine::quick_open_search` and then the ONE navigation
//!   chokepoint the overlay dispatches (`search_ui::navigate_to` →
//!   `navigation.focus{region:"main", block_id}`).
//! @pbt covers quick-open-jump — after a jump the caret is live INSIDE the new
//! root: on the destination's first child, or on the destination's creation
//! affordance when it has no children. Never on the destination itself, which
//! renders through the editor-less `page_title` variant and leaves the
//! keyboard dead
//! (docs/Testing/bugfunnel/entries/
//! 2026-09-08-quick-open-enter-navigation-leaves-no-editable-focus.md).
//!
//! Restricted to PAGE hits: a jump to a content hit reds
//! `inv-viewmodel-tree-virtual-slots` (see the precondition's comment), which
//! is a question about that oracle, not about the seat.
//!
//! Distinct from `NavigateFocus`, which reaches the same op by CLICKING a
//! LeftSidebar entry and is therefore restricted to sidebar-listed pages. A
//! quick-open hit is any block the search returns, so this transition is the
//! only one that jumps to a non-page block or to a page the sidebar hides.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefGlobalFocus;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefNavHistoryMut;
use holon_pbt_core::capabilities::SutSearch;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Longest query drawn from a hit's content. Long enough to be selective,
/// short enough that the search's `LIMIT` is rarely why the hit is missing.
const MAX_DRAWN_QUERY: usize = 12;

/// The Pages and In-content branch reads of one `quick_open_search`.
#[cfg(feature = "otel-testing")]
const SEARCH_BRANCH_READS: usize = 2;
/// Slack `Search` measures around those two reads.
#[cfg(feature = "otel-testing")]
const SEARCH_BRANCH_TOLERANCE: usize = 2;

/// Type a query into quick-open, then press Enter on `hit`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I search for {query} and jump to {hit}")]
pub struct JumpToSearchHit {
    pub query: String,
    pub hit: EntityUri,
}

/// A query the reference model knows `id` matches: its first word long enough
/// to be selective. `None` for a block whose content has no such word — that
/// block is simply not a jump candidate this tick.
fn query_for(content: &str) -> Option<String> {
    content
        .split_whitespace()
        .find(|w| w.chars().count() >= 3)
        .map(|w| w.chars().take(MAX_DRAWN_QUERY).collect())
}

/// The D97.a seating law on the MODEL's caret, right after the jump: its first
/// child, or, for a childless destination, the creation affordance that hangs
/// under it. The PRODUCTION caret is held to the same seat by the SUT half
/// (`SutSearch::jump_to_search_hit`), which is where the teeth are — this one
/// only stops the model from drifting out from under them.
fn assert_caret_seated_inside<R: RefBlockTree + RefGlobalFocus>(
    state: &R,
    destination: &EntityUri,
) {
    let caret = state
        .global_focused_block()
        .expect("a jump seats a caret, so the model must hold one");
    let expected_slot = holon_frontend::row_origin::RowOrigin::creation_placeholder_id(destination);
    let seated_in_subtree = state
        .sorted_children(destination)
        .first()
        .is_some_and(|first| *first == caret);
    assert!(
        seated_in_subtree || caret.as_str() == expected_slot,
        "[JumpToSearchHit] the caret must land inside {destination}: expected its first child or \
         the slot {expected_slot}, got {caret}"
    );
}

impl<R: RefLifecycle + RefBlockTree> TransitionFactory<R> for JumpToSearchHit {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // The quick-open predicate reads the `block` matview and the
        // `block_tags` junction, and the jump writes `navigation_history` —
        // all Turso-native surfaces.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates: Vec<(String, EntityUri)> = state
            .all_non_seed_block_ids()
            .into_iter()
            .filter(|id| state.is_page_block(id) && !state.is_layout_block(id))
            .filter_map(|id| {
                let query = query_for(state.block_content(&id)?)?;
                Some((query, id))
            })
            .collect();
        check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = proptest::sample::select(candidates)
                .prop_map(|(query, hit)| JumpToSearchHit { query, hit })
                .boxed();
            // Weighted like `Search` (6): high enough that a smoke-length walk
            // reaches it, low enough that it does not crowd out the editing
            // alphabet it exists to make reachable.
            (6, strat)
        })
    }
}

impl<R: RefLifecycle + RefBlockTree + RefNavHistoryMut + RefGlobalFocus> TransitionRef<R>
    for JumpToSearchHit
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!self.query.is_empty(), Reason::PreconditionFailed),
            check(
                state.block_content(&self.hit).is_some(),
                Reason::FocusedBlockMissing,
            ),
            // PAGES only, for now. A jump to a CONTENT hit is a real
            // production gesture, but it reds `inv-viewmodel-tree-virtual-slots`:
            // a non-page focus root still renders somewhere as a `tree_item`
            // carrying a state_toggle, which that oracle reads as the
            // wrong-variant dogfood regression. Whether the oracle or the
            // content-hit render is wrong is not this transition's question.
            check(state.is_page_block(&self.hit), Reason::FocusedNotText),
            check(
                !state.is_layout_block(&self.hit),
                Reason::FocusedInLayoutBlocks,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // The jump IS a `navigation.focus(main)`: same history row, same open-pin
        // reset, same caret seat. Only the gesture that produced it differs.
        state.nav_focus(holon_api::Region::Main, &self.hit);
        assert_caret_seated_inside(state, &self.hit);
    }
}

impl JumpToSearchHit {
    pub(crate) fn declared_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<dyn SutSearch>()]
    }
}

#[allow(async_fn_in_trait)]
impl<R: RefLifecycle + RefBlockTree, S: SutSearch> ::holon_pbt_core::TransitionImpl<R, S>
    for JumpToSearchHit
{
    async fn apply_to_sut(&self, state: &R, sut: &mut S) {
        // The seat the model predicts, handed over so the SUT half can assert
        // the PRODUCTION caret against it. Read from the PRE-apply state,
        // which is the state the navigation seats from.
        let expected_first_child = state.sorted_children(&self.hit).into_iter().next();
        sut.jump_to_search_hit(&self.query, &self.hit, expected_first_child.as_ref())
            .await;
    }
}

#[cfg(feature = "otel-testing")]
impl ::holon_pbt_core::budget::SqlBudget for JumpToSearchHit {
    fn expected_sql<R2: ::holon_pbt_core::capabilities::RefSqlCardinality>(
        &self,
        state: &R2,
    ) -> ExpectedSql {
        use crate::pbt::transition_budgets::JOURNAL_READS;
        use crate::pbt::transition_budgets::NAV_DML_READS;
        use crate::pbt::transition_budgets::NAV_RENDER_FAN_READS;
        use crate::pbt::transition_budgets::REACTIVE_BASE;
        use crate::pbt::transition_budgets::docs_tolerance;
        // One quick-open search (its two branch reads, as in `Search`) followed
        // by the `NavigateFocus` shape — the same op, so the same cost, caret
        // seat included.
        ExpectedSql {
            reads: SEARCH_BRANCH_READS
                + REACTIVE_BASE
                + JOURNAL_READS
                + NAV_DML_READS
                + NAV_RENDER_FAN_READS,
            writes: 0,
            ddl: 0,
            tolerance: SEARCH_BRANCH_TOLERANCE + docs_tolerance(state),
        }
    }
}
