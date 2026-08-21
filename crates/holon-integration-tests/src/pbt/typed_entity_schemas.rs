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

/// One free-standing type's comparison shape: the columns the oracle predicts
/// and the invariant reads back off the matview, `id` first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedEntitySchema {
    pub type_name: String,
    /// The primary-key column (`id` for every type today).
    pub id_column: String,
    /// The remaining persisted columns, in a stable order. A create fills every
    /// one of them, so no column compares NULL-vs-empty-string.
    pub value_columns: Vec<String>,
}

impl TypedEntitySchema {
    /// The full column list the SUT read and the oracle rows share.
    pub fn columns(&self) -> Vec<String> {
        let mut cols = vec![self.id_column.clone()];
        cols.extend(self.value_columns.iter().cloned());
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
                    .filter(|f| !f.primary_key)
                    .map(|f| f.name.clone())
                    .collect();
                TypedEntitySchema {
                    type_name: type_def.name.clone(),
                    id_column,
                    value_columns,
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
