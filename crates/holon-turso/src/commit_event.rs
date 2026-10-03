use std::sync::Arc;

use holon_core::storage::StorageEntity;
use turso_core::types::RelationChangeEvent;

use crate::turso::TursoBackend;

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

/// The changes one commit made to one relation.
pub struct CommitEvent<'e>(&'e RelationChangeEvent);

impl<'e> CommitEvent<'e> {
    pub fn new(event: &'e RelationChangeEvent) -> Self {
        CommitEvent(event)
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
    /// decode.
    pub fn changes(&self) -> Result<Vec<CommitChange>, String> {
        let columns: Vec<Arc<str>> = self
            .0
            .columns
            .iter()
            .map(|c| Arc::from(c.as_str()))
            .collect();
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
                let row =
                    TursoBackend::parse_row_values_with_schema(&values, &columns).map_err(|e| {
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
