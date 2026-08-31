//! `RefTypedEntities` — the datatype-axis oracle read cap (BG-1).
//!
//! @pbt kind ref
//! @pbt covers typed-matview-matches-ref — the free-standing entities the
//!   oracle created, per type, compared against each type's matview.
//!
//! Reads the per-type fragment the datatype transitions maintain. The type set
//! is whatever has been DECLARED — registry-seeded plus runtime-drawn — never a
//! name written here. Nothing
//! else in the model consults it: a free-standing type has no block/tree
//! coupling, so no other expectation may vary with these sets.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::RefTypedEntities;

use super::super::reference_state::ReferenceState;

impl RefTypedEntities for ReferenceState {
    fn typed_entity_schemas(&self) -> Vec<(String, Vec<String>)> {
        // DECLARED types, not a static list: the registry's free-standing types
        // seed the set and `DeclareTypedSchema` adds runtime-drawn ones, so a
        // generated schema is covered the moment it is declared.
        self.typed_entities
            .declared()
            .map(|(type_name, _)| {
                let mut columns = vec!["id".to_string()];
                columns.extend(self.typed_entities.columns(type_name));
                (type_name.clone(), columns)
            })
            .collect()
    }

    fn expected_typed_entity_rows(&self, type_name: &str) -> Vec<Vec<String>> {
        self.typed_entities.rows(type_name)
    }

    fn typed_entity_ids(&self) -> BTreeSet<String> {
        self.typed_entities.all_ids().cloned().collect()
    }
}
