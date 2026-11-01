//! What each relation in this database declares, as the engine reports it.
//!
//! A cache, never an interpretation: the entries are written from `PRAGMA
//! table_info`, so a statement that created nothing changes nothing here, and a
//! relation's answer moves only when the engine's answer moves.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::sql_parser::normalize_table_name;

/// The column vocabulary of every relation the database holds.
///
/// Asking it replaces every hand-kept list of "tables that have column X".
/// It is filled by the database actor, which re-derives a relation from the
/// engine after any DDL that could have touched it.
#[derive(Debug, Default)]
pub struct SchemaCatalog {
    /// Lower-cased relation name to its lower-cased column names, each with
    /// its declared type.
    relations: RwLock<HashMap<String, HashMap<String, String>>>,
}

/// One column as the engine reports it. A column declared without a type
/// reports the empty string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogColumn {
    pub name: String,
    pub declared_type: String,
}

impl From<String> for CatalogColumn {
    fn from(name: String) -> Self {
        CatalogColumn {
            name,
            declared_type: String::new(),
        }
    }
}

impl SchemaCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the engine reported `relation` to this catalog.
    pub fn knows(&self, relation: &str) -> bool {
        self.relations
            .read()
            .expect("schema catalog poisoned")
            .contains_key(&key(relation))
    }

    /// Whether `relation` declares `column`.
    ///
    /// A relation the engine does not have declares nothing: a CTE name, a
    /// subquery alias, and a dropped table all answer `false`.
    pub fn declares_column(&self, relation: &str, column: &str) -> bool {
        self.relations
            .read()
            .expect("schema catalog poisoned")
            .get(&key(relation))
            .is_some_and(|cols| cols.contains_key(&column.to_lowercase()))
    }

    /// The declared type of each of `columns` of `relation`, in order; `None`
    /// for a column the relation does not declare.
    pub fn declared_types(
        &self,
        relation: &str,
        columns: &[impl AsRef<str>],
    ) -> Vec<Option<String>> {
        let relations = self.relations.read().expect("schema catalog poisoned");
        let declared = relations.get(&key(relation));
        columns
            .iter()
            .map(|c| declared.and_then(|cols| cols.get(&c.as_ref().to_lowercase()).cloned()))
            .collect()
    }

    /// Record what the engine reports for one relation.
    pub fn set_columns(
        &self,
        relation: &str,
        columns: impl IntoIterator<Item = impl Into<CatalogColumn>>,
    ) {
        self.relations
            .write()
            .expect("schema catalog poisoned")
            .insert(key(relation), by_name(columns));
    }

    /// Forget a relation the engine no longer has.
    pub fn forget(&self, relation: &str) {
        self.relations
            .write()
            .expect("schema catalog poisoned")
            .remove(&key(relation));
    }

    /// Replace the whole catalog with a freshly derived schema. Used when a
    /// statement's footprint could not be read, so nothing may be assumed to
    /// have survived it.
    pub fn replace_all(&self, relations: impl IntoIterator<Item = (String, Vec<CatalogColumn>)>) {
        let fresh = relations
            .into_iter()
            .map(|(name, columns)| (key(&name), by_name(columns)))
            .collect();
        *self.relations.write().expect("schema catalog poisoned") = fresh;
    }

    /// The relations this catalog knows, for diagnostics and tests.
    pub fn known_relations(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .relations
            .read()
            .expect("schema catalog poisoned")
            .keys()
            .cloned()
            .collect();
        names.sort_unstable();
        names
    }
}

fn key(relation: &str) -> String {
    normalize_table_name(relation).to_lowercase()
}

fn by_name(columns: impl IntoIterator<Item = impl Into<CatalogColumn>>) -> HashMap<String, String> {
    columns
        .into_iter()
        .map(|c| {
            let c = c.into();
            (c.name.to_lowercase(), c.declared_type)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_relation_answers_for_its_columns() {
        let catalog = SchemaCatalog::new();
        catalog.set_columns("widget", ["id".to_string(), "_change_origin".to_string()]);

        assert!(catalog.declares_column("widget", "_change_origin"));
        assert!(catalog.declares_column("WIDGET", "_CHANGE_ORIGIN"));
        assert!(!catalog.declares_column("widget", "absent"));
    }

    #[test]
    fn declared_types_answer_per_column_in_order() {
        let catalog = SchemaCatalog::new();
        catalog.set_columns(
            "t",
            [CatalogColumn {
                name: "Count".to_string(),
                declared_type: "TEXT_FMT_tsl_count".to_string(),
            }],
        );

        assert_eq!(
            catalog.declared_types("T", &["absent", "count"]),
            vec![None, Some("TEXT_FMT_tsl_count".to_string())]
        );
        assert_eq!(
            catalog.declared_types("never_created", &["count"]),
            vec![None]
        );
    }

    #[test]
    fn an_unrecorded_relation_declares_nothing() {
        let catalog = SchemaCatalog::new();
        assert!(!catalog.declares_column("never_created", "_change_origin"));
    }

    #[test]
    fn forget_removes_the_relation() {
        let catalog = SchemaCatalog::new();
        catalog.set_columns("widget", ["_change_origin".to_string()]);
        catalog.forget("widget");

        assert!(!catalog.declares_column("widget", "_change_origin"));
    }

    #[test]
    fn replace_all_drops_relations_the_new_schema_does_not_name() {
        let catalog = SchemaCatalog::new();
        catalog.set_columns("gone", ["_change_origin".to_string()]);
        catalog.replace_all([(
            "kept".to_string(),
            vec!["_change_origin".to_string().into()],
        )]);

        assert!(!catalog.declares_column("gone", "_change_origin"));
        assert!(catalog.declares_column("kept", "_change_origin"));
        assert_eq!(catalog.known_relations(), vec!["kept".to_string()]);
    }
}
