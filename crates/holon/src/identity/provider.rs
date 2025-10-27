//! IdentityProvider — operation surface for entity identity and proposals.
//!
//! Implements `OperationProvider` for the `identity` entity. All operations
//! flow through `OperationDispatcher` and are logged by `OperationLogObserver`,
//! so undo/redo replay works with the inverse `Operation` returned in each
//! `OperationResult::new(...)`. Internal undo primitives
//! (`restore_canonical_after_merge`, `delete_proposal`, `revert_proposal_status`)
//! are listed in `operations()` so the dispatcher can route inverse executions;
//! they are not intended as user-facing surfaces.
//!
//! Inverses are designed to be deterministic: every operation captures the
//! complete prior state in its inverse params, and inverse of inverse is the
//! original (so redo replays cleanly). `propose_merge` requires an explicit
//! `id` to keep insertion idempotent under replay.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::core::datasource::{OperationProvider, OperationResult, Result};
use crate::storage::DbHandle;
use holon_api::{EntityName, Operation, OperationDescriptor, OperationParam, TypeHint, Value};
use holon_core::storage::types::StorageEntity;

pub const ENTITY_NAME: &str = "identity";
pub const SHORT_NAME: &str = "identity";

/// Snapshot of one alias row, captured during `merge_entities` and
/// JSON-serialized into the inverse operation's params.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AliasSnapshot {
    system: String,
    foreign_id: String,
    confidence: f64,
}

/// IdentityProvider — entity identity operations backed by the
/// `canonical_entity`, `entity_alias`, `proposal_queue` tables.
pub struct IdentityProvider {
    db_handle: DbHandle,
}

impl IdentityProvider {
    pub fn new(db_handle: DbHandle) -> Self {
        Self { db_handle }
    }

    // -----------------------------------------------------------------
    // merge_entities + restore_canonical_after_merge
    // -----------------------------------------------------------------

    async fn merge_entities(
        &self,
        canonical_a: &str,
        canonical_b: &str,
    ) -> Result<OperationResult> {
        if canonical_a == canonical_b {
            return Err(
                format!("merge_entities: canonical_a == canonical_b ({canonical_a})").into(),
            );
        }

        // 1. Snapshot canonical_a's row.
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(canonical_a.to_string()));
        let canonical_rows = self
            .db_handle
            .query(
                "SELECT kind, primary_label, created_at FROM canonical_entity WHERE id = $id",
                params.clone(),
            )
            .await
            .map_err(|e| format!("merge_entities: select canonical_a: {e}"))?;

        let canonical_a_row = canonical_rows
            .into_iter()
            .next()
            .ok_or_else(|| format!("merge_entities: canonical_a '{canonical_a}' not found"))?;

        // 2. Verify canonical_b exists (otherwise the UPDATE silently rewrites
        //    aliases to a phantom canonical_id).
        let mut b_params = HashMap::new();
        b_params.insert("id".to_string(), Value::String(canonical_b.to_string()));
        let b_rows = self
            .db_handle
            .query(
                "SELECT 1 AS ok FROM canonical_entity WHERE id = $id",
                b_params,
            )
            .await
            .map_err(|e| format!("merge_entities: select canonical_b: {e}"))?;
        if b_rows.is_empty() {
            return Err(format!("merge_entities: canonical_b '{canonical_b}' not found").into());
        }

        // 3. Snapshot the alias rows that point to canonical_a.
        let alias_rows = self
            .db_handle
            .query(
                "SELECT system, foreign_id, confidence FROM entity_alias \
                 WHERE canonical_id = $id",
                params.clone(),
            )
            .await
            .map_err(|e| format!("merge_entities: select aliases: {e}"))?;

        let aliases: Vec<AliasSnapshot> = alias_rows
            .iter()
            .map(|row| AliasSnapshot {
                system: extract_string(row, "system"),
                foreign_id: extract_string(row, "foreign_id"),
                confidence: extract_f64(row, "confidence"),
            })
            .collect();

        // 4. Rewrite aliases A -> B.
        let mut update_params = HashMap::new();
        update_params.insert("a".to_string(), Value::String(canonical_a.to_string()));
        update_params.insert("b".to_string(), Value::String(canonical_b.to_string()));
        self.db_handle
            .query(
                "UPDATE entity_alias SET canonical_id = $b WHERE canonical_id = $a",
                update_params,
            )
            .await
            .map_err(|e| format!("merge_entities: update aliases: {e}"))?;

        // 5. Delete the merged-from canonical.
        self.db_handle
            .query(
                "DELETE FROM canonical_entity WHERE id = $id",
                params.clone(),
            )
            .await
            .map_err(|e| format!("merge_entities: delete canonical_a: {e}"))?;

        // 6. Build the inverse operation.
        let aliases_json = serde_json::to_string(&aliases)
            .map_err(|e| format!("merge_entities: serialize alias snapshot: {e}"))?;

        let mut inverse_params = HashMap::new();
        inverse_params.insert("id".to_string(), Value::String(canonical_a.to_string()));
        inverse_params.insert(
            "kind".to_string(),
            canonical_a_row
                .get("kind")
                .cloned()
                .unwrap_or(Value::String(String::new())),
        );
        inverse_params.insert(
            "primary_label".to_string(),
            canonical_a_row
                .get("primary_label")
                .cloned()
                .unwrap_or(Value::String(String::new())),
        );
        inverse_params.insert(
            "created_at".to_string(),
            canonical_a_row
                .get("created_at")
                .cloned()
                .unwrap_or(Value::Integer(0)),
        );
        inverse_params.insert(
            "merged_into_id".to_string(),
            Value::String(canonical_b.to_string()),
        );
        inverse_params.insert("alias_keys_json".to_string(), Value::String(aliases_json));

        let inverse = Operation::new(
            ENTITY_NAME,
            "restore_canonical_after_merge",
            "Restore canonical after merge",
            inverse_params,
        );

        Ok(OperationResult::new(Vec::new(), inverse))
    }

    async fn restore_canonical_after_merge(
        &self,
        params: &StorageEntity,
    ) -> Result<OperationResult> {
        let id = require_string(params, "id")?;
        let kind = require_string(params, "kind")?;
        let primary_label = require_string(params, "primary_label")?;
        let created_at = require_i64(params, "created_at")?;
        let merged_into_id = require_string(params, "merged_into_id")?;
        let alias_keys_json = require_string(params, "alias_keys_json")?;

        let aliases: Vec<AliasSnapshot> = serde_json::from_str(&alias_keys_json)
            .map_err(|e| format!("restore_canonical_after_merge: parse alias snapshot: {e}"))?;

        // 1. Re-insert canonical row.
        let mut insert_params = HashMap::new();
        insert_params.insert("id".to_string(), Value::String(id.clone()));
        insert_params.insert("kind".to_string(), Value::String(kind));
        insert_params.insert("primary_label".to_string(), Value::String(primary_label));
        insert_params.insert("created_at".to_string(), Value::Integer(created_at));
        self.db_handle
            .query(
                "INSERT INTO canonical_entity (id, kind, primary_label, created_at) \
                 VALUES ($id, $kind, $primary_label, $created_at)",
                insert_params,
            )
            .await
            .map_err(|e| format!("restore_canonical_after_merge: insert canonical: {e}"))?;

        // 2. Rewrite aliases back to the restored canonical.
        for alias in &aliases {
            let mut alias_params = HashMap::new();
            alias_params.insert("id".to_string(), Value::String(id.clone()));
            alias_params.insert("system".to_string(), Value::String(alias.system.clone()));
            alias_params.insert(
                "foreign_id".to_string(),
                Value::String(alias.foreign_id.clone()),
            );
            alias_params.insert("confidence".to_string(), Value::Float(alias.confidence));
            self.db_handle
                .query(
                    "UPDATE entity_alias SET canonical_id = $id, confidence = $confidence \
                     WHERE system = $system AND foreign_id = $foreign_id",
                    alias_params,
                )
                .await
                .map_err(|e| format!("restore_canonical_after_merge: update alias: {e}"))?;
        }

        // 3. Inverse: merge_entities(canonical_a=id, canonical_b=merged_into_id).
        let mut inverse_params = HashMap::new();
        inverse_params.insert("canonical_a".to_string(), Value::String(id));
        inverse_params.insert("canonical_b".to_string(), Value::String(merged_into_id));
        let inverse = Operation::new(
            ENTITY_NAME,
            "merge_entities",
            "Merge entities",
            inverse_params,
        );

        Ok(OperationResult::new(Vec::new(), inverse))
    }

    // -----------------------------------------------------------------
    // propose_merge / delete_proposal
    // -----------------------------------------------------------------

    async fn propose_merge(&self, params: &StorageEntity) -> Result<OperationResult> {
        let id = require_i64(params, "id")?;
        let kind = require_string(params, "kind")?;
        let evidence_json = require_string(params, "evidence_json")?;
        let created_at = require_i64(params, "created_at")?;

        let mut insert = HashMap::new();
        insert.insert("id".to_string(), Value::Integer(id));
        insert.insert("kind".to_string(), Value::String(kind));
        insert.insert("evidence_json".to_string(), Value::String(evidence_json));
        insert.insert("created_at".to_string(), Value::Integer(created_at));
        self.db_handle
            .query(
                "INSERT INTO proposal_queue (id, kind, evidence_json, status, created_at) \
                 VALUES ($id, $kind, $evidence_json, 'pending', $created_at)",
                insert,
            )
            .await
            .map_err(|e| format!("propose_merge: insert: {e}"))?;

        let mut inverse_params = HashMap::new();
        inverse_params.insert("id".to_string(), Value::Integer(id));
        let inverse = Operation::new(
            ENTITY_NAME,
            "delete_proposal",
            "Delete proposal",
            inverse_params,
        );

        Ok(OperationResult::new(Vec::new(), inverse))
    }

    async fn delete_proposal(&self, params: &StorageEntity) -> Result<OperationResult> {
        let id = require_i64(params, "id")?;

        // Snapshot the row.
        let mut sel = HashMap::new();
        sel.insert("id".to_string(), Value::Integer(id));
        let rows = self
            .db_handle
            .query(
                "SELECT kind, evidence_json, status, created_at \
                 FROM proposal_queue WHERE id = $id",
                sel.clone(),
            )
            .await
            .map_err(|e| format!("delete_proposal: select: {e}"))?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| format!("delete_proposal: id {id} not found"))?;

        // Delete.
        self.db_handle
            .query("DELETE FROM proposal_queue WHERE id = $id", sel)
            .await
            .map_err(|e| format!("delete_proposal: delete: {e}"))?;

        // Inverse: restore_proposal carries the full row including status,
        // so undo restores any state (pending / accepted / rejected) exactly.
        let mut inverse_params = HashMap::new();
        inverse_params.insert("id".to_string(), Value::Integer(id));
        inverse_params.insert(
            "kind".to_string(),
            Value::String(extract_string(&row, "kind")),
        );
        inverse_params.insert(
            "evidence_json".to_string(),
            Value::String(extract_string(&row, "evidence_json")),
        );
        inverse_params.insert(
            "status".to_string(),
            Value::String(extract_string(&row, "status")),
        );
        inverse_params.insert(
            "created_at".to_string(),
            Value::Integer(extract_i64(&row, "created_at")),
        );

        Ok(OperationResult::new(
            Vec::new(),
            Operation::new(
                ENTITY_NAME,
                "restore_proposal",
                "Restore proposal",
                inverse_params,
            ),
        ))
    }

    async fn restore_proposal(&self, params: &StorageEntity) -> Result<OperationResult> {
        let id = require_i64(params, "id")?;
        let kind = require_string(params, "kind")?;
        let evidence_json = require_string(params, "evidence_json")?;
        let status = require_string(params, "status")?;
        let created_at = require_i64(params, "created_at")?;

        let mut insert = HashMap::new();
        insert.insert("id".to_string(), Value::Integer(id));
        insert.insert("kind".to_string(), Value::String(kind));
        insert.insert("evidence_json".to_string(), Value::String(evidence_json));
        insert.insert("status".to_string(), Value::String(status));
        insert.insert("created_at".to_string(), Value::Integer(created_at));
        self.db_handle
            .query(
                "INSERT INTO proposal_queue (id, kind, evidence_json, status, created_at) \
                 VALUES ($id, $kind, $evidence_json, $status, $created_at)",
                insert,
            )
            .await
            .map_err(|e| format!("restore_proposal: insert: {e}"))?;

        let mut inverse_params = HashMap::new();
        inverse_params.insert("id".to_string(), Value::Integer(id));
        Ok(OperationResult::new(
            Vec::new(),
            Operation::new(
                ENTITY_NAME,
                "delete_proposal",
                "Delete proposal",
                inverse_params,
            ),
        ))
    }

    // -----------------------------------------------------------------
    // accept_proposal / reject_proposal / revert_proposal_status
    // -----------------------------------------------------------------

    async fn set_proposal_status(&self, id: i64, new_status: &str) -> Result<OperationResult> {
        // Snapshot current status.
        let mut sel = HashMap::new();
        sel.insert("id".to_string(), Value::Integer(id));
        let rows = self
            .db_handle
            .query(
                "SELECT status FROM proposal_queue WHERE id = $id",
                sel.clone(),
            )
            .await
            .map_err(|e| format!("set_proposal_status: select: {e}"))?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| format!("set_proposal_status: id {id} not found"))?;
        let old_status = extract_string(&row, "status");

        // Update.
        let mut upd = HashMap::new();
        upd.insert("id".to_string(), Value::Integer(id));
        upd.insert("status".to_string(), Value::String(new_status.to_string()));
        self.db_handle
            .query(
                "UPDATE proposal_queue SET status = $status WHERE id = $id",
                upd,
            )
            .await
            .map_err(|e| format!("set_proposal_status: update: {e}"))?;

        // Inverse: revert_proposal_status restoring old.
        let mut inverse_params = HashMap::new();
        inverse_params.insert("id".to_string(), Value::Integer(id));
        inverse_params.insert("status".to_string(), Value::String(old_status));
        let inverse = Operation::new(
            ENTITY_NAME,
            "revert_proposal_status",
            "Revert proposal status",
            inverse_params,
        );
        Ok(OperationResult::new(Vec::new(), inverse))
    }

    // -----------------------------------------------------------------
    // OperationDescriptor helpers
    // -----------------------------------------------------------------

    fn descriptor(
        name: &str,
        display_name: &str,
        description: &str,
        required: Vec<OperationParam>,
    ) -> OperationDescriptor {
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "id".to_string(),
            name: name.to_string(),
            display_name: display_name.to_string(),
            description: description.to_string(),
            required_params: required,
            affected_fields: Vec::new(),
            param_mappings: Vec::new(),
            ..Default::default()
        }
    }

    fn string_param(name: &str, description: &str) -> OperationParam {
        OperationParam {
            name: name.to_string(),
            type_hint: TypeHint::String,
            description: description.to_string(),
        }
    }

    fn integer_param(name: &str, description: &str) -> OperationParam {
        OperationParam {
            name: name.to_string(),
            type_hint: TypeHint::Number,
            description: description.to_string(),
        }
    }
}

#[async_trait]
impl OperationProvider for IdentityProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![
            // -- User-facing --
            Self::descriptor(
                "merge_entities",
                "Merge entities",
                "Merge canonical_a into canonical_b: rewrite aliases and delete the merged-from canonical.",
                vec![
                    Self::string_param(
                        "canonical_a",
                        "Canonical id to merge from (will be deleted)",
                    ),
                    Self::string_param("canonical_b", "Canonical id to merge into"),
                ],
            ),
            Self::descriptor(
                "propose_merge",
                "Propose merge",
                "Append a merge proposal to proposal_queue (status='pending').",
                vec![
                    Self::integer_param(
                        "id",
                        "Proposal id (caller-provided to keep replay deterministic)",
                    ),
                    Self::string_param("kind", "Proposal kind (e.g., 'merge')"),
                    Self::string_param("evidence_json", "JSON-encoded evidence payload"),
                    Self::integer_param("created_at", "Creation timestamp (ms since epoch)"),
                ],
            ),
            Self::descriptor(
                "accept_proposal",
                "Accept proposal",
                "Mark a proposal as accepted.",
                vec![Self::integer_param("id", "Proposal id")],
            ),
            Self::descriptor(
                "reject_proposal",
                "Reject proposal",
                "Mark a proposal as rejected.",
                vec![Self::integer_param("id", "Proposal id")],
            ),
            // -- Internal undo primitives (registered so the dispatcher routes
            //    inverse executions; not intended as user surfaces). --
            Self::descriptor(
                "restore_canonical_after_merge",
                "Restore canonical after merge",
                "Internal: undo of merge_entities. Re-inserts canonical and rewrites aliases.",
                vec![
                    Self::string_param("id", "Canonical id to restore"),
                    Self::string_param("kind", "Original kind"),
                    Self::string_param("primary_label", "Original primary label"),
                    Self::integer_param("created_at", "Original created_at"),
                    Self::string_param(
                        "merged_into_id",
                        "The canonical_b that the original merge targeted",
                    ),
                    Self::string_param(
                        "alias_keys_json",
                        "JSON snapshot of [{system, foreign_id, confidence}]",
                    ),
                ],
            ),
            Self::descriptor(
                "delete_proposal",
                "Delete proposal",
                "Internal: undo of propose_merge / restore_proposal. Removes a row from proposal_queue.",
                vec![Self::integer_param("id", "Proposal id")],
            ),
            Self::descriptor(
                "restore_proposal",
                "Restore proposal",
                "Internal: undo of delete_proposal. Re-inserts a proposal row with original status.",
                vec![
                    Self::integer_param("id", "Proposal id"),
                    Self::string_param("kind", "Proposal kind"),
                    Self::string_param("evidence_json", "Evidence payload"),
                    Self::string_param("status", "Original status"),
                    Self::integer_param("created_at", "Original created_at"),
                ],
            ),
            Self::descriptor(
                "revert_proposal_status",
                "Revert proposal status",
                "Internal: self-inverse of accept_proposal / reject_proposal. Sets a specific status.",
                vec![
                    Self::integer_param("id", "Proposal id"),
                    Self::string_param("status", "Status to set"),
                ],
            ),
        ]
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        if entity_name != ENTITY_NAME {
            return Err(format!(
                "IdentityProvider: expected entity '{ENTITY_NAME}', got '{entity_name}'"
            )
            .into());
        }

        match op_name {
            "merge_entities" => {
                let canonical_a = require_string(&params, "canonical_a")?;
                let canonical_b = require_string(&params, "canonical_b")?;
                self.merge_entities(&canonical_a, &canonical_b).await
            }
            "restore_canonical_after_merge" => self.restore_canonical_after_merge(&params).await,
            "propose_merge" => self.propose_merge(&params).await,
            "delete_proposal" => self.delete_proposal(&params).await,
            "restore_proposal" => self.restore_proposal(&params).await,
            "accept_proposal" => {
                let id = require_i64(&params, "id")?;
                self.set_proposal_status(id, "accepted").await
            }
            "reject_proposal" => {
                let id = require_i64(&params, "id")?;
                self.set_proposal_status(id, "rejected").await
            }
            "revert_proposal_status" => {
                let id = require_i64(&params, "id")?;
                let status = require_string(&params, "status")?;
                self.set_proposal_status(id, &status).await
            }
            other => Err(format!("IdentityProvider: unknown op '{other}'").into()),
        }
    }
}

// -----------------------------------------------------------------------
// Param helpers
// -----------------------------------------------------------------------

fn require_string(params: &StorageEntity, key: &str) -> Result<String> {
    match params.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(format!("expected string for '{key}', got {other:?}").into()),
        None => Err(format!("missing required param '{key}'").into()),
    }
}

fn require_i64(params: &StorageEntity, key: &str) -> Result<i64> {
    match params.get(key) {
        Some(Value::Integer(n)) => Ok(*n),
        Some(other) => Err(format!("expected integer for '{key}', got {other:?}").into()),
        None => Err(format!("missing required param '{key}'").into()),
    }
}

fn extract_string(row: &StorageEntity, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn extract_i64(row: &StorageEntity, key: &str) -> i64 {
    match row.get(key) {
        Some(Value::Integer(n)) => *n,
        _ => 0,
    }
}

fn extract_f64(row: &StorageEntity, key: &str) -> f64 {
    match row.get(key) {
        Some(Value::Float(f)) => *f,
        Some(Value::Integer(n)) => *n as f64,
        _ => 0.0,
    }
}
