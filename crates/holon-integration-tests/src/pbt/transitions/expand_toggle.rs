//! Transition: expand a collapsed `expand_toggle` widget.
//!
//! Production behavior: the user clicks the chevron on an `expand_toggle`.
//! GPUI's `expand_toggle` builder flips its `expanded: Mutable<bool>` to
//! `true`; on the next render `LazyReactiveSlot::materialize_if_gated`
//! fires the captured thunk and surfaces the body. Subsequent collapse +
//! re-expand reuse the cache (see `LazyReactiveSlot` in
//! `holon_frontend::reactive_view_model`).
//!
//! Two candidate sources:
//!
//! 1. Explicit render-expression toggles: blocks whose render expression
//!    mentions `expand_toggle` AND are currently collapsed. The generator first
//!    checks these (original path).
//!
//! 2. Profile-driven toggles: non-seed page blocks that are strict descendants
//!    of the main focus root (e.g. `embedded_page` profile variant wraps them
//!    in `expand_toggle` with lazy live_query content). The generator
//!    enumerates these via `RefBlockTree` without depending on
//!    `render_expr_mentions` (which never matches profile-driven toggles).
//!
//! The two paths differ in apply semantics:
//! - Explicit toggles: `set_expanded` also models `block.collapsed`.
//! - Profile-driven: `set_expanded_view_local` — view-local only, no document
//!   `collapsed` field mutation.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefRenderExpr;
use holon_pbt_core::capabilities::RefToggleMut;
use holon_pbt_core::capabilities::SutBlockInteract;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ExpandToggle {
    pub block_id: EntityUri,
}

impl<R> TransitionFactory<R> for ExpandToggle
where
    R: RefLifecycle + RefRenderExpr + RefToggleMut + RefBlockTree,
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Source 1: explicit render-expression toggles (original path).
        let explicit: Vec<EntityUri> = state
            .render_expr_ids()
            .into_iter()
            .filter(|uri| {
                state.render_expr_mentions(uri, "expand_toggle") && !state.is_expanded(uri)
            })
            .collect();

        // Source 2: profile-driven toggles — non-seed pages that are strict
        // descendants of the main focus root and not already expanded.
        let main_roots = state.focus_root_ids(CapRegion::Main);
        let profile: Vec<EntityUri> = if main_roots.is_empty() {
            vec![]
        } else {
            state
                .all_non_seed_block_ids()
                .into_iter()
                .filter(|id| {
                    state.is_page_block(id)
                        && state.is_descendant_of_any(id, &main_roots)
                        && !main_roots.contains(id)
                        && !state.is_expanded(id)
                })
                .collect()
        };

        let mut candidates = explicit;
        candidates.extend(profile);
        candidates.sort();
        candidates.dedup();

        check(!candidates.is_empty(), Reason::NoExpandToggleCandidates).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|block_id| ExpandToggle { block_id })
                .boxed();
            (1, strat)
        })
    }
}

impl<R> TransitionRef<R> for ExpandToggle
where
    R: RefLifecycle + RefRenderExpr + RefToggleMut + RefBlockTree,
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let has_explicit_toggle = state.has_render_expr(&self.block_id)
            && state.render_expr_mentions(&self.block_id, "expand_toggle");
        let is_profile_target = {
            let main_roots = state.focus_root_ids(CapRegion::Main);
            state.is_page_block(&self.block_id)
                && state.is_descendant_of_any(&self.block_id, &main_roots)
                && !main_roots.contains(&self.block_id)
        };
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                has_explicit_toggle || is_profile_target,
                Reason::FocusedBlockMissing,
            ),
            check(
                !state.is_expanded(&self.block_id),
                Reason::ToggleAlreadyExpanded,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        let has_explicit_toggle = state.has_render_expr(&self.block_id)
            && state.render_expr_mentions(&self.block_id, "expand_toggle");
        if has_explicit_toggle {
            state.set_expanded(&self.block_id, true);
        } else {
            state.set_expanded_view_local(&self.block_id, true);
        }
    }
}

crate::cap_transition! {
    ExpandToggle: SutBlockInteract,
    where R: [ RefLifecycle + RefRenderExpr + RefToggleMut + RefBlockTree ],
    |me, _state, sut| {
        sut.expand_toggle(&me.block_id).await;
    }
    sql_budget: |_me, state| {
        let update = crate::pbt::transition_budgets::expected_sql_for_kind(
            crate::pbt::transition_budgets::MutationKind::Update,
            state.active_watch_count(),
            state.block_count(),
            state.document_count(),
        );
        ExpectedSql {
            reads: REACTIVE_BASE + update.reads,
            writes: update.writes,
            ddl: 0,
            tolerance: docs_tolerance(state) + update.tolerance,
        }
    }
}
