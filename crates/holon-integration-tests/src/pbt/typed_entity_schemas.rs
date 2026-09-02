//! The free-standing datatypes the keystone's datatype axis exercises (BG-1),
//! read FROM THE REGISTRY rather than named in the test.
//!
//! Nothing here mentions `person`. The axis draws whatever free-standing types
//! the running configuration declares; today that set happens to be `{person}`,
//! and it grows without touching a transition, an invariant, or a cap. The
//! selection predicate mirrors the DI provider's `is_free_standing`
//! (`crates/holon/src/di/schema_providers.rs`) so the oracle expects exactly
//! the serializations production actually creates — if the two ever drift, the
//! matview-vs-oracle invariant reports it rather than hiding it.
//!
//! End state (see the design doc's keystone/OQ-6 section): these schemas become
//! GENERATED — the keystone declares a random small schema at runtime,
//! registers it through the adapter, and asserts convergence. This module is
//! the seam that swap lands on; the hand-authored registry types are the seed.

use std::sync::LazyLock;

use holon_api::ComputedTier;
use holon_api::computation::Computation;

/// One free-standing type's comparison shape: the columns the oracle predicts
/// and the invariant reads back off the matview, `id` first.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedEntitySchema {
    pub type_name: String,
    /// The primary-key column (`id` for every type today).
    pub id_column: String,
    /// The persisted columns whose cells the ROW'S AUTHOR writes
    /// ([`ColumnValueKind::Declared`]), in a stable order. A create fills every
    /// one of them, so no column compares NULL-vs-empty-string.
    ///
    /// An engine-owned column — the overflow bag and its kind map — is
    /// deliberately absent: the engine stamps `_provenance` into the bag on
    /// every create, so its stored value is not the value any author wrote and
    /// the oracle cannot predict it from the transition alone.
    pub value_columns: Vec<String>,
    /// The type's `computed_persisted` fields, each with the `Computation` the
    /// registry compiled it into. The SUT reads these off the matview as
    /// planted columns; the oracle predicts them by EVALUATING the same
    /// `Computation` — so the invariant compares the two lowerings of one
    /// declaration rather than restating the SQL.
    pub computed_columns: Vec<(String, Computation)>,
}

impl TypedEntitySchema {
    /// The full column list the SUT read and the oracle rows share: the key,
    /// the stored columns, then the planted computed ones.
    pub fn columns(&self) -> Vec<String> {
        let mut cols = vec![self.id_column.clone()];
        cols.extend(self.value_columns.iter().cloned());
        cols.extend(self.computed_columns.iter().map(|(n, _)| n.clone()));
        cols
    }
}

/// Every free-standing type the default configuration declares, in a stable
/// order. Computed once — the registry is immutable for a keystone run.
pub fn free_standing_schemas() -> &'static [TypedEntitySchema] {
    static SCHEMAS: LazyLock<Vec<TypedEntitySchema>> = LazyLock::new(|| {
        let registry =
            holon_profiles::create_default_registry().expect("default TypeRegistry for the axis");
        let mut schemas: Vec<TypedEntitySchema> = registry
            .all()
            .into_iter()
            .filter(is_free_standing)
            .map(|type_def| {
                let persisted = type_def.persistent_fields();
                let id_column = persisted
                    .iter()
                    .find(|f| f.primary_key)
                    .map(|f| f.name.clone())
                    .unwrap_or_else(|| {
                        panic!(
                            "free-standing type '{}' has no primary-key column",
                            type_def.name
                        )
                    });
                let value_columns = persisted
                    .iter()
                    .filter(|f| !f.primary_key && !f.value_kind.is_engine_owned())
                    .map(|f| f.name.clone())
                    .collect();
                let computed_columns = type_def
                    .computed_specs()
                    .into_iter()
                    .filter(|(_, spec)| spec.tier() == ComputedTier::ComputedPersisted)
                    .map(|(name, spec)| (name.to_string(), spec.computation().clone()))
                    .collect();
                TypedEntitySchema {
                    type_name: type_def.name.clone(),
                    id_column,
                    value_columns,
                    computed_columns,
                }
            })
            .collect();
        schemas.sort_by(|a, b| a.type_name.cmp(&b.type_name));
        assert!(
            !schemas.is_empty(),
            "the datatype axis needs at least one free-standing type; the registry declared none, \
             so CreateTypedEntity could never fire and the matview invariant would pass vacuously"
        );
        schemas
    });
    &SCHEMAS
}

/// Mirrors `holon::di::schema_providers::is_free_standing`.
fn is_free_standing(type_def: &holon_api::TypeDefinition) -> bool {
    type_def.id_references.is_none()
        && !type_def.persistent_fields().is_empty()
        && type_def.name != "block"
}
