//! Transition: run `*::rebuild_views`, the maintenance op that drops every
//! Turso watch view, recreates the ones a live watch listens to, and sends
//! each watch what its rebuilt view gained and lost.
//!
//! @pbt rung mcp
//!   drives the MCP `execute_operation` tool, the entry point an agent uses;
//!   no UI binding reaches `rebuild_views`.
//! @pbt covers watch-views-survive-rebuild — a watch opened before a
//!   `rebuild_views` keeps receiving changes after it and holds what its
//!   rebuilt view holds

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutRebuildViews;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;

/// `MatviewManager::rebuild_watch_views` lists the watch views once. The
/// rebuild runs one actor turn per view and issues no SQL span, so no term
/// grows with the number of views; its cost is wall time.
#[cfg(feature = "otel-testing")]
const WATCH_VIEW_LIST_READS: usize = 1;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("an agent rebuilds the views")]
pub struct RebuildViews;

impl<R: RefLifecycle> TransitionFactory<R> for RebuildViews {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        RebuildViews
            .preconditions(state)
            .map(|_| (1, Just(RebuildViews).boxed()))
    }
}

impl<R: RefLifecycle> TransitionRef<R> for RebuildViews {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        check(state.app_started(), Reason::AppNotStarted)
    }

    fn apply_to_ref(&self, _: &mut R) {
        // Views are a projection of the model's state, which a rebuild does
        // not change.
    }
}

crate::cap_transition! {
    RebuildViews: SutRebuildViews,
    where R: [ RefLifecycle ],
    |_me, _state, sut| {
        sut.rebuild_views().await;
    }
    sql_budget: |_me, _state| {
        ExpectedSql {
            reads: REACTIVE_BASE + WATCH_VIEW_LIST_READS,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}
