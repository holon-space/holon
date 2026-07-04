//! Transition: expand a collapsed `expand_toggle` widget.
//!
//! Production behavior: the user clicks the chevron on an `expand_toggle`.
//! GPUI's `expand_toggle` builder flips its `expanded: Mutable<bool>` to
//! `true`; on the next render `LazyReactiveSlot::materialize_if_gated`
//! fires the captured thunk and surfaces the body. Subsequent collapse +
//! re-expand reuse the cache (see `LazyReactiveSlot` in
//! `holon_frontend::reactive_view_model`).
//!
//! Candidate set: blocks whose render expression mentions `expand_toggle`
//! AND are currently collapsed. Today the default fixtures don't produce
//! any such blocks (claude-history.yaml / GitHub.org-style integrations
//! are out of scope for the PBT corpus), so the generator routinely
//! rejects with `NoExpandToggleCandidates`. The skeleton is here so the
//! transition activates the moment a fixture grows an expand_toggle
//! render — no follow-up wiring needed at the state-machine level. SUT
//! plumbing is live (`E2ESut::set_expand_toggle_gate` walks the engine's
//! reactive tree and flips `.expanded`); it fails loud if the corpus
//! produces a toggle render but the engine yields no matching node.

use holon_api::EntityUri;
use holon_pbt_core::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use holon_pbt_core::capabilities::{RefLifecycle, RefRenderExpr, RefToggleMut, SutBlockInteract};
use holon_pbt_core::{TransitionFactory, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE, docs_tolerance};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ExpandToggle {
    pub block_id: EntityUri,
}

impl<R: RefLifecycle + RefRenderExpr + RefToggleMut> TransitionFactory<R> for ExpandToggle {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates: Vec<EntityUri> = state
            .render_expr_ids()
            .into_iter()
            .filter(|uri| {
                state.render_expr_mentions(uri, "expand_toggle") && !state.is_expanded(uri)
            })
            .collect();
        check(!candidates.is_empty(), Reason::NoExpandToggleCandidates).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|block_id| ExpandToggle { block_id })
                .boxed();
            // Weight 1 — until expand_toggle-bearing fixtures land,
            // candidates are always empty; weight is academic. When the
            // corpus grows toggles, raise alongside pin/click.
            (1, strat)
        })
    }
}

impl<R: RefLifecycle + RefRenderExpr + RefToggleMut> TransitionRef<R> for ExpandToggle {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.has_render_expr(&self.block_id),
                Reason::FocusedBlockMissing,
            ),
        ];
        if state.has_render_expr(&self.block_id) {
            checks.push(check(
                state.render_expr_mentions(&self.block_id, "expand_toggle"),
                Reason::PreconditionFailed,
            ));
        }
        checks.push(check(
            !state.is_expanded(&self.block_id),
            Reason::ToggleAlreadyExpanded,
        ));
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.set_expanded(&self.block_id, true);
    }
}

crate::cap_transition! {
    ExpandToggle: SutBlockInteract,
    where R: [ RefLifecycle + RefRenderExpr + RefToggleMut ],
    |me, _state, sut| {
        sut.expand_toggle(&me.block_id).await;
    }
    sql_budget: |_me, state| {
        // Pure frontend-state flip: no SQL traffic. The reactive base
        // captures any incidental watcher activity.
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}
