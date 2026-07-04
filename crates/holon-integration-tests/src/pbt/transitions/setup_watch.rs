//! Transition: set up a query watch (post-startup).
//!
//! @pbt rung dispatch
//!   `register_watch` compiles + registers the query watcher directly (no
//!   UI gesture for registering a watch).
//! @pbt covers watch-register — query watch registration + CDC subscription
//!
//! Mirrors the legacy logic split across `state_machine.rs:519-532`
//! (generator), `state_machine.rs:3160` (precondition),
//! `state_machine.rs:2203-2215` (ref-state apply),
//! `sut.rs:1302-1317` (SUT apply), and
//! `transition_budgets.rs:152-163` (expected SQL).

use holon_api::QueryLanguage;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefWatchesMut;
use holon_pbt_core::capabilities::SutWatchRegister;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::query::TestQuery;
use crate::pbt::query::WatchSpec;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// Set up a new query watch.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SetupWatch {
    pub query_id: String,
    pub query: TestQuery,
    pub language: QueryLanguage,
}

impl<R: RefLifecycle + RefWatchesMut<WatchSpec = WatchSpec>> TransitionFactory<R> for SetupWatch {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: the navigation / CDC-watch / MCP providers this transition
        // dispatches have no Loro-native source in the no-Turso wiring
        // (see loro_block_query_source.rs:77). Gate it out of {Loro} slices.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Create a dummy instance to validate preconditions (no query/language needed
        // yet); if they fail, the generator is disabled entirely (no weight=0
        // fallback).
        let dummy = SetupWatch {
            query_id: "dummy".to_string(),
            query: TestQuery {
                table: crate::pbt::query::QueryTable::Blocks,
                columns: vec!["id".to_string()],
                predicates: vec![],
                source: crate::pbt::query::QuerySource::AllBlocks,
            },
            language: QueryLanguage::HolonSql,
        };
        dummy.preconditions(state).map(|_| {
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
            (1, strat)
        })
    }
}

impl<R: RefLifecycle + RefWatchesMut<WatchSpec = WatchSpec>> TransitionRef<R> for SetupWatch {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.insert_watch(
            &self.query_id,
            WatchSpec {
                query: self.query.clone(),
                language: self.language,
            },
        );
    }
}

crate::cap_transition! {
    SetupWatch: SutWatchRegister,
    where R: [ RefLifecycle + RefWatchesMut ],
    |me, _state, sut| {
        // Compile the integration-test-local `TestQuery` at the boundary: the
        // `SutWatchRegister` cap (pbt-core) cannot name `TestQuery`, so it takes
        // the already-compiled `(source, lang)` — exactly what `E2ESut`'s old
        // `apply_setup_watch` did internally before this decomposition.
        let (source, lang) = me.query.compile_for(me.language);
        sut.register_watch(&me.query_id, &source, lang).await;
    }
    sql_budget: |_me, state| {
        let blocks = state.block_count();
        let _docs = state.document_count();
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
