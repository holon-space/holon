//! Transition: set up a query watch (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:519-532` (generator),
//! `state_machine.rs:3160` (precondition),
//! `state_machine.rs:2203-2215` (ref-state apply),
//! `sut.rs:1302-1317` (SUT apply), and
//! `transition_budgets.rs:152-163` (expected SQL).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_capabilities::RefWatchesMut;
use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::capabilities::{RefLifecycle, SutWatchRegister};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE, docs_tolerance};

use holon_api::QueryLanguage;

use crate::pbt::query::TestQuery;

/// Set up a new query watch.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SetupWatch {
    pub query_id: String,
    pub query: TestQuery,
    pub language: QueryLanguage,
}

impl TransitionFactory<ReferenceState> for SetupWatch {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutWatchRegister,
        >()]
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: the navigation / CDC-watch / MCP providers this transition
        // dispatches have no Loro-native source in the no-Turso wiring
        // (see loro_block_query_source.rs:77). Gate it out of {Loro} slices.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Create a dummy instance to validate preconditions (no query/language needed yet);
        // if they fail, the generator is disabled entirely (no weight=0 fallback).
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

impl<R: RefLifecycle + RefWatchesMut> TransitionRef<R> for SetupWatch {
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
        state.register_watch(self.query_id.clone(), self.query.clone(), self.language);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutWatchRegister> TransitionImpl<ReferenceState, S> for SetupWatch {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        // Compile the integration-test-local `TestQuery` at the boundary: the
        // `SutWatchRegister` cap (pbt-core) cannot name `TestQuery`, so it takes
        // the already-compiled `(source, lang)` — exactly what `E2ESut`'s old
        // `apply_setup_watch` did internally before this decomposition.
        let (source, lang) = self.query.compile_for(self.language);
        sut.register_watch(&self.query_id, &source, lang).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for SetupWatch {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let blocks = state.domain.block_state.blocks.len();
        let _docs = state.files.documents.len();
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
