use std::sync::Arc;

use holon_core::storage::StorageEntity;
use turso_core::types::RelationChangeEvent;

use crate::schema_catalog::SchemaCatalog;
use crate::turso::TursoBackend;
use crate::turso::column_formats;

/// Where an event sits among the events one commit delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitEnvelope {
    pub commit_id: u64,
    pub index: u32,
    pub len: u32,
}

/// One row change, as the row image the change carries.
#[derive(Debug, Clone, PartialEq)]
pub enum CommitChange {
    Upsert(StorageEntity),
    Delete(StorageEntity),
}

/// The changes one commit made to one relation, read with the column types
/// `catalog` holds for it.
pub struct CommitEvent<'e>(&'e RelationChangeEvent, &'e SchemaCatalog);

impl<'e> CommitEvent<'e> {
    pub fn new(event: &'e RelationChangeEvent, catalog: &'e SchemaCatalog) -> Self {
        CommitEvent(event, catalog)
    }

    pub fn envelope(&self) -> CommitEnvelope {
        CommitEnvelope {
            commit_id: self.0.commit_id,
            index: self.0.commit_index,
            len: self.0.commit_len,
        }
    }

    pub fn relation(&self) -> &str {
        &self.0.relation_name
    }

    /// The changes in commit order; the error names the first that does not
    /// decode, or the relation when the catalog does not know it.
    pub fn changes(&self) -> Result<Vec<CommitChange>, String> {
        if !self.1.knows(&self.0.relation_name) {
            return Err(format!(
                "{}: the relation is unknown to the schema catalog, so its cells cannot be typed",
                self.0.relation_name
            ));
        }
        let columns: Vec<Arc<str>> = self
            .0
            .columns
            .iter()
            .map(|c| Arc::from(c.as_str()))
            .collect();
        let declared = self.1.declared_types(&self.0.relation_name, &columns);
        let formats = column_formats(&columns, declared.iter().map(Option::as_deref))
            .map_err(|e| format!("{}: {e}", self.0.relation_name))?;
        self.0
            .changes
            .iter()
            .map(|change| {
                let values = change.parse_record().map_err(|e| {
                    format!(
                        "{} rowid={}: the record does not decode: {e}",
                        self.0.relation_name, change.id
                    )
                })?;
                let row = TursoBackend::parse_row_values_with_schema(&values, &columns, &formats)
                    .map_err(|e| {
                    format!(
                        "{} rowid={}: the row does not parse: {e}",
                        self.0.relation_name, change.id
                    )
                })?;
                Ok(match change.change {
                    turso_core::types::DatabaseChangeType::Delete { .. } => {
                        CommitChange::Delete(row)
                    }
                    turso_core::types::DatabaseChangeType::Insert { .. }
                    | turso_core::types::DatabaseChangeType::Update { .. } => {
                        CommitChange::Upsert(row)
                    }
                })
            })
            .collect()
    }
}
