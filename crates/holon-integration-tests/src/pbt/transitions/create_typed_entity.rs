//! Transition: create a FREE-STANDING typed entity — the datatype axis
//! (block-generalization increment 1, BG-1).
//!
//! @pbt rung datatype
//! @pbt covers typed-matview-matches-ref — a free-standing typed entity is
//!   created through the mode's write authority, lives in NO block table, and
//!   its matview matches the oracle.
//! @pbt covers typed-loro-matches-ref — in every Loro mode the entity lands in
//!   the Loro typed-entity store, with or without Turso.
//!
//! A free-standing entity is NOT block-shaped: no parent, no tree, no org
//! projection. This transition creates one through `SutTypedEntityWrite` — a
//! dispatch by entity name, so SqlOnly reaches the SQL provider and every Loro
//! mode reaches the Loro store — and records it in the per-type oracle
//! fragment.
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
use holon_pbt_core::capabilities::SutTypedEntityWrite;
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
        // Every storage mode has a typed-entity write authority: Turso in
        // SqlOnly, the Loro store in every Loro mode. No `Actor::UI`: the
        // composed keystone is headless and strips that actor.
        RequiredWiring::any_storage_of([StorageAdapter::Turso, StorageAdapter::Loro])
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
                    // A fresh, deterministic id unique within the type,
                    // reproducible under proptest replay unlike a uuid — and
                    // scheme-qualified, because a create SUPPLIES an entity
                    // reference and the operation boundary refuses one that
                    // names no entity. It is also the id the row is STORED
                    // under, so the oracle and the matview compare one string.
                    let scheme = type_name.replace('_', "-");
                    let id = format!("{scheme}:{scheme}-{existing}");
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
    CreateTypedEntity: SutTypedEntityWrite,
    where R: [ RefLifecycle ],
    |me, _state, sut| {
        sut.create_typed_entity(&me.type_name, &me.id, me.field_pairs()).await;
    }
    sql_budget: |_me, _state| {
        // One INSERT into `<type>_raw`, by the SQL provider or by the Loro
        // projector. Turso IVM maintains the matview internally; a generous
        // tolerance absorbs that maintenance + any CDC-driven reads.
        ExpectedSql {
            reads: 0,
            writes: 1,
            ddl: 0,
            tolerance: 64,
        }
    }
}
