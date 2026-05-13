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

use crate::pbt::validation::{Reason, check};
use holon_api::EntityUri;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::value_fn_invariants::rhai_mentions;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE, docs_tolerance};

#[derive(Clone, Debug)]
pub struct ExpandToggle {
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for ExpandToggle {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates: Vec<EntityUri> = state
            .render_expressions
            .iter()
            .filter(|(uri, expr)| {
                rhai_mentions(expr, "expand_toggle") && !state.expanded_toggles.contains(*uri)
            })
            .map(|(uri, _)| uri.clone())
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

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for ExpandToggle {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(
                state.render_expressions.contains_key(&self.block_id),
                Reason::FocusedBlockMissing,
            ),
        ];
        if let Some(expr) = state.render_expressions.get(&self.block_id) {
            checks.push(check(
                rhai_mentions(expr, "expand_toggle"),
                Reason::PreconditionFailed,
            ));
        }
        checks.push(check(
            !state.expanded_toggles.contains(&self.block_id),
            Reason::ToggleAlreadyExpanded,
        ));
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.expanded_toggles.insert(self.block_id.clone());
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_expand_toggle(&self.block_id).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
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
