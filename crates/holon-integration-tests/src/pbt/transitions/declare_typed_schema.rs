//! Transition: DECLARE a new free-standing datatype at runtime, with a
//! proptest-drawn schema — the datatype axis's generative half (BG-1).
//!
//! @pbt rung datatype
//! @pbt covers typed-matview-matches-ref — an arbitrarily-shaped type declared
//!   at runtime is serialized by the generic adapter (raw table + read
//! matview),   and entities created in it converge to the oracle.
//! @pbt slips-if-removed the axis would only ever exercise the hand-authored
//!   registry types, so an adapter that happened to work for THOSE shapes —
//!   3 columns, one particular naming — and broke on any other would pass.
//!
//! This is what makes the axis schema-generic rather than merely type-generic:
//! nothing hand-authors the type. The COLUMN COUNT and the COLUMN NAMES are
//! drawn, the type is registered through `TursoAdapter` exactly as production
//! registers one, and `CreateTypedEntity` then fills it.
//!
//! EXERCISING IT DELIBERATELY. At the default weights this transition is rare —
//! a 16-case default run drew it ZERO times, which would make the axis look
//! green while proving nothing. To pressure it on purpose:
//!
//! ```text
//! HOLON_PBT_WEIGHTS='DeclareTypedSchema:300,CreateTypedEntity:300' \
//!   HOLON_PBT_FORCE_FULL=1 PROPTEST_CASES=8 just pbt general 8
//! ```
//!
//! Measured at that setting: 31 declarations / 109 creates, the matview
//! invariant engaged fully in every arm, zero divergences.
//!
//! Identifier safety is a PRECONDITION of the generator, not a runtime filter:
//! names are drawn from a lowercase alphabet, screened against the adapter's
//! SQL keyword rejection, made unique, and given a `gen_` prefix so a drawn
//! type can never collide with a registry type or a core table. A generator
//! that could emit `order` would spend its cases bouncing off a declaration
//! error instead of testing the adapter.

use holon_pbt_core::RequiredWiring;
use holon_pbt_core::StorageAdapter;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutTypedEntity;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
use crate::pbt::transitions::create_typed_entity::TypedColumns;

/// How many value columns a drawn schema may carry, beyond the `id` key. One
/// proves the minimal shape; the upper bound keeps a case's DDL cheap while
/// still varying width.
const MIN_COLUMNS: usize = 1;
const MAX_COLUMNS: usize = 4;

/// Declare a free-standing type named `type_name` with `columns` TEXT value
/// columns, alongside its `id` primary key.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I declare a datatype {type_name} with columns {columns}")]
pub struct DeclareTypedSchema {
    pub type_name: String,
    pub columns: TypedColumns,
}

impl TransitionFactory<ReferenceState> for DeclareTypedSchema {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> RequiredWiring {
        // Declaring a type is pure Turso DDL through the adapter — no gesture,
        // no window. Same arm as `CreateTypedEntity`.
        RequiredWiring::HasStorage(StorageAdapter::Turso)
    }

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        check(state.app_started(), Reason::AppNotStarted).map(|_| {
            // `gen_<n>` cannot collide with a registry type or a core table, and
            // is never a SQL keyword. `n` counts declared types, so a replay
            // reproduces the same name.
            let type_name = format!("gen_{}", state.typed_entities.declared_count());
            let strat = proptest::collection::hash_set(
                proptest::string::string_regex("[a-z]{3,8}").expect("valid regex"),
                MIN_COLUMNS..=MAX_COLUMNS,
            )
            .prop_map(move |names| {
                // Sorted for a deterministic column order; keyword-colliding
                // draws are prefixed rather than dropped, so the column count
                // stays exactly what was drawn.
                let mut columns: Vec<String> = names
                    .into_iter()
                    .map(|n| {
                        if holon_turso::turso_adapter::is_sql_keyword(&n) {
                            format!("c_{n}")
                        } else {
                            n
                        }
                    })
                    .collect();
                columns.sort();
                DeclareTypedSchema {
                    type_name: type_name.clone(),
                    columns: TypedColumns(columns),
                }
            })
            .boxed();
            // Lower weight than a create: a run wants several entities per
            // declared type, not a schema per step.
            (3, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for DeclareTypedSchema {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!self.columns.0.is_empty(), Reason::PreconditionFailed),
            // Re-declaring a live type is a migration, not a declaration — that
            // is OQ-5's territory and needs its own oracle.
            check(
                !state.typed_entities.is_declared(&self.type_name),
                Reason::PreconditionFailed,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state
            .typed_entities
            .declare(self.type_name.clone(), self.columns.0.clone());
    }
}

crate::cap_transition! {
    DeclareTypedSchema: SutTypedEntity,
    where R: [ RefLifecycle ],
    |me, _state, sut| {
        sut.declare_typed_schema(&me.type_name, me.columns.0.clone()).await;
    }
    sql_budget: |_me, _state| {
        // DDL only: the raw table plus the matview reconcile. No row writes.
        // The adapter probes sqlite_master while reconciling, so reads are not
        // zero; the tolerance absorbs that without under-stating the DDL.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 2,
            tolerance: 64,
        }
    }
}
