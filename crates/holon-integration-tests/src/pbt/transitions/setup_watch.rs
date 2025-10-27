//! Transition: set up a query watch (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:519-532` (generator),
//! `state_machine.rs:3160` (precondition),
//! `state_machine.rs:2203-2215` (ref-state apply),
//! `sut.rs:1302-1317` (SUT apply), and
//! `transition_budgets.rs:152-163` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::query::WatchSpec;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE, docs_tolerance};

use holon_api::QueryLanguage;

use crate::pbt::query::TestQuery;

/// Set up a new query watch.
#[derive(Clone, Debug)]
pub struct SetupWatch {
    pub query_id: String,
    pub query: TestQuery,
    pub language: QueryLanguage,
}

impl E2ETransitionFactory for SetupWatch {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }
        let strat = (
            crate::pbt::generators::generate_test_query(),
            crate::pbt::generators::generate_query_language(),
            "[a-z]{1,10}",
        )
            .prop_map(|(query, language, id)| SetupWatch {
                query_id: format!("query-{}", id),
                query,
                language,
            })
            .boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for SetupWatch {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        state.app_started
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.active_watches.insert(
            self.query_id.clone(),
            WatchSpec {
                query: self.query.clone(),
                language: self.language,
            },
        );
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_setup_watch(&self.query_id, &self.query, self.language)
            .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let blocks = state.block_state.blocks.len();
        let _docs = state.documents.len();
        // reactive base (5) + view existence check (2) + turso internal check (1)
        //   + initial matview data read (1) = 9 reads, 0 writes, 1 DDL.
        // Pending CDC events from prior transitions drain during SetupWatch,
        // adding reactive cycles proportional to the number of dirtied blocks.
        // Pathological cases (CreateStaleLoro disrupting StartApp init) defer
        // matview creation here, requiring large ddl/write tolerance.
        ExpectedSql {
            reads: REACTIVE_BASE + 2 + 1 + 1,
            writes: 0,
            ddl: 1,
            tolerance: docs_tolerance(state) + blocks * 6,
        }
    }
}
