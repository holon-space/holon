//! Transition: create a FREE-STANDING typed entity — the datatype axis
//! (block-generalization increment 1, BG-1).
//!
//! @pbt rung datatype
//! @pbt covers typed-matview-matches-ref — a free-standing typed entity is
//!   created through its Turso serialization (raw table + read matview), lives
//!   in NO block table, and its matview matches the oracle.
//!
//! A free-standing entity is NOT block-shaped: no parent, no tree, no org
//! projection. It serializes to its own `<type>_raw` write table + `<type>`
//! read matview (the `TursoAdapter` derivation). This transition creates one
//! through `SutTypedEntity` and records it in the per-type oracle fragment;
//! `inv-typed-matview-matches-ref` then asserts every type's matview equals the
//! oracle AND that no typed-entity id reaches a block table.
//!
//! The TYPE IS DRAWN, never named here: the generator picks from the registry's
//! free-standing schemas, and the field values come from that schema's
//! persisted columns. Declaring another free-standing type widens this
//! transition's reach with no edit — which is the point of the axis.

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

/// A schema's value-column names, in order. A newtype so it can travel through
/// a replay step as compact JSON.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TypedColumns(pub Vec<String>);

holon_pbt_core::step_field_via_json!(
    TypedColumns,
    vec![
        TypedColumns(vec!["email".to_string()]),
        TypedColumns(vec!["a".to_string(), "b".to_string()]),
    ]
);

/// The persisted `(column, value)` pairs a create writes, as ONE unit — the
/// pairing cannot desync the way two parallel vectors can, and it travels
/// through a replay step as compact JSON.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TypedFieldValues(pub Vec<(String, String)>);

holon_pbt_core::step_field_via_json!(
    TypedFieldValues,
    vec![
        TypedFieldValues(Vec::new()),
        TypedFieldValues(vec![("email".to_string(), "abc".to_string())]),
    ]
);

/// Create one free-standing entity of `type_name`, filling every persisted
/// value column the drawn schema declares.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I create a {type_name} entity {id} with {fields}")]
pub struct CreateTypedEntity {
    pub type_name: String,
    pub id: String,
    /// The persisted value columns and their values, in the schema's order.
    pub fields: TypedFieldValues,
}

impl CreateTypedEntity {
    /// The `(column, value)` pairs (excluding the primary key) handed to the
    /// SUT.
    fn field_pairs(&self) -> Vec<(String, String)> {
        self.fields.0.clone()
    }

    /// The value cells in schema order — what the oracle records as the row.
    fn values(&self) -> Vec<String> {
        self.fields.0.iter().map(|(_, v)| v.clone()).collect()
    }
}

impl TransitionFactory<ReferenceState> for CreateTypedEntity {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> RequiredWiring {
        // The serialization (raw table + matview) is pure Turso: the write goes
        // straight to `<type>_raw`, with no gesture and no window. Requiring
        // `Actor::UI` here would make the transition UNREACHABLE — the composed
        // keystone is headless by construction and strips that actor
        // (`wide_e2e::set_for_wiring`). The frontend arm is already selected for
        // by the `SutTypedEntity` cap.
        RequiredWiring::HasStorage(StorageAdapter::Turso)
    }

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let declared: Vec<(String, Vec<String>, usize)> = state
            .typed_entities
            .declared()
            .map(|(name, cols)| (name.clone(), cols.clone(), state.typed_entities.count(name)))
            .collect();
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            // Nothing to create into until a type is declared. The registry's
            // free-standing types seed this, so it is normally non-empty; a
            // configuration declaring none deselects honestly rather than
            // generating writes against a table that does not exist.
            check(!declared.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            let strat = proptest::sample::select(declared)
                .prop_flat_map(|(type_name, columns, existing)| {
                    // A fresh, deterministic id unique within the type
                    // (`<type>-N`), reproducible under proptest replay unlike a
                    // uuid.
                    let id = format!("{type_name}-{existing}");
                    // Short org-safe ASCII words: no quotes/markup, round-trip
                    // clean.
                    proptest::collection::vec(
                        proptest::string::string_regex("[a-z]{1,6}").expect("valid regex"),
                        columns.len()..=columns.len(),
                    )
                    .prop_map(move |values| CreateTypedEntity {
                        type_name: type_name.clone(),
                        id: id.clone(),
                        fields: TypedFieldValues(columns.iter().cloned().zip(values).collect()),
                    })
                })
                .boxed();
            // Low-moderate weight: a real datatype axis alongside block edits,
            // not so high it starves the block/editing alphabet.
            (8, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for CreateTypedEntity {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!self.id.is_empty(), Reason::PreconditionFailed),
            // NOTE: this makes a zero-persisted-field free-standing type
            // silently UNDRAWABLE — the schema-set non-emptiness assert counts
            // TYPES, not reachable draws, so such a type would look covered
            // while never being created. No such type exists in Inc 1.
            check(!self.fields.0.is_empty(), Reason::PreconditionFailed),
            check(
                state.typed_entities.is_declared(&self.type_name),
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
            .add(self.type_name.clone(), self.id.clone(), self.values());
    }
}

crate::cap_transition! {
    CreateTypedEntity: SutTypedEntity,
    where R: [ RefLifecycle ],
    |me, _state, sut| {
        sut.create_typed_entity(&me.type_name, &me.id, me.field_pairs()).await;
    }
    sql_budget: |_me, _state| {
        // One raw INSERT into `<type>_raw` (no op-pipeline). Turso IVM maintains
        // the matview internally; a generous tolerance absorbs that maintenance
        // + any CDC-driven reads without under-stating the write.
        ExpectedSql {
            reads: 0,
            writes: 1,
            ddl: 0,
            tolerance: 64,
        }
    }
}
