//! Turso-based CommandLog implementation
//!
//! Provides persistent undo/redo via the commands table.

use async_trait::async_trait;
use serde_json;
use tracing;
use turso;

use crate::storage::DbHandle;
use crate::storage::types::{Result, StorageEntity, StorageError};
use crate::sync::command_log::{CommandEntry, CommandLog, CommandStatus, SyncStatus};
use crate::sync::event_bus::CommandId;
use holon_api::{Operation, Value};

/// Turso-based CommandLog implementation
pub struct TursoCommandLog {
    db_handle: DbHandle,
}

impl TursoCommandLog {
    /// Create a new TursoCommandLog
    pub fn new(db_handle: DbHandle) -> Self {
        Self { db_handle }
    }

    /// Parse a StorageEntity (from db_handle query) into a CommandEntry
    fn parse_command_entity(entity: &StorageEntity) -> Result<CommandEntry> {
        let get_string = |key: &str| -> Result<String> {
            match entity.get(key) {
                Some(Value::String(s)) => Ok(s.clone()),
                _ => Err(StorageError::DatabaseError(format!(
                    "Expected string for {}",
                    key
                ))),
            }
        };

        let get_optional_string = |key: &str| -> Result<Option<String>> {
            match entity.get(key) {
                Some(Value::String(s)) => Ok(Some(s.clone())),
                Some(Value::Null) | None => Ok(None),
                _ => Err(StorageError::DatabaseError(format!(
                    "Expected string or null for {}",
                    key
                ))),
            }
        };

        let get_i64 = |key: &str| -> Result<i64> {
            match entity.get(key) {
                Some(Value::Integer(i)) => Ok(*i),
                _ => Err(StorageError::DatabaseError(format!(
                    "Expected integer for {}",
                    key
                ))),
            }
        };

        let get_optional_i64 = |key: &str| -> Result<Option<i64>> {
            match entity.get(key) {
                Some(Value::Integer(i)) => Ok(Some(*i)),
                Some(Value::Null) | None => Ok(None),
                _ => Err(StorageError::DatabaseError(format!(
                    "Expected integer or null for {}",
                    key
                ))),
            }
        };

        let id = get_string("id")?;
        let operation_json = get_string("operation")?;
        let inverse_json = get_optional_string("inverse")?;
        let display_name = get_string("display_name")?;
        let entity_type = get_string("entity_type")?;
        let entity_id = get_string("entity_id")?;
        let target_system = get_optional_string("target_system")?;
        let status_str = get_string("status")?;
        let sync_status_str = get_string("sync_status")?;
        let created_at = get_i64("created_at")?;
        let executed_at = get_optional_i64("executed_at")?;
        let synced_at = get_optional_i64("synced_at")?;
        let undone_at = get_optional_i64("undone_at")?;
        let error_details = get_optional_string("error_details")?;
        let undone_by_command_id = get_optional_string("undone_by_command_id")?;
        let undoes_command_id = get_optional_string("undoes_command_id")?;

        let operation: Operation = serde_json::from_str(&operation_json).map_err(|e| {
            StorageError::SerializationError(format!("Failed to deserialize operation: {}", e))
        })?;

        let inverse = inverse_json
            .map(|json| {
                serde_json::from_str(&json).map_err(|e| {
                    StorageError::SerializationError(format!(
                        "Failed to deserialize inverse: {}",
                        e
                    ))
                })
            })
            .transpose()?;

        let status = CommandStatus::from_str(&status_str).ok_or_else(|| {
            StorageError::DatabaseError(format!("Invalid status: {}", status_str))
        })?;

        let sync_status = SyncStatus::from_str(&sync_status_str).ok_or_else(|| {
            StorageError::DatabaseError(format!("Invalid sync_status: {}", sync_status_str))
        })?;

        Ok(CommandEntry {
            id,
            operation,
            inverse,
            display_name,
            entity_type,
            entity_id,
            target_system,
            status,
            sync_status,
            created_at,
            executed_at,
            synced_at,
            undone_at,
            error_details,
            undone_by_command_id,
            undoes_command_id,
        })
    }

    /// Initialize the commands table schema
    pub async fn init_schema(&self) -> Result<()> {
        // Create unified commands table (replaces both operations and command_sourcing commands)
        self.db_handle
            .execute_ddl(
                "CREATE TABLE IF NOT EXISTS commands (
                id TEXT PRIMARY KEY,
                operation TEXT NOT NULL,
                inverse TEXT,
                display_name TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                target_system TEXT,
                status TEXT DEFAULT 'executed',
                sync_status TEXT DEFAULT 'local',
                created_at INTEGER NOT NULL,
                executed_at INTEGER,
                synced_at INTEGER,
                undone_at INTEGER,
                error_details TEXT,
                undone_by_command_id TEXT,
                undoes_command_id TEXT
            )",
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to create commands table: {}", e))
            })?;

        // Index for undo stack (most recent executed commands)
        self.db_handle
            .execute_ddl(
                "CREATE INDEX IF NOT EXISTS idx_commands_undo_stack
             ON commands(created_at DESC)
             WHERE status = 'executed' AND inverse IS NOT NULL",
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to create undo stack index: {}", e))
            })?;

        // Index for redo stack (recently undone commands)
        self.db_handle
            .execute_ddl(
                "CREATE INDEX IF NOT EXISTS idx_commands_redo_stack
             ON commands(undone_at DESC)
             WHERE status = 'undone' AND inverse IS NOT NULL",
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to create redo stack index: {}", e))
            })?;

        // Index for pending sync
        self.db_handle
            .execute_ddl(
                "CREATE INDEX IF NOT EXISTS idx_commands_pending_sync
             ON commands(created_at)
             WHERE sync_status = 'pending_sync'",
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to create pending sync index: {}", e))
            })?;

        // Index for entity history
        self.db_handle
            .execute_ddl(
                "CREATE INDEX IF NOT EXISTS idx_commands_entity
             ON commands(entity_type, entity_id, created_at)",
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to create entity index: {}", e))
            })?;

        tracing::info!("[TursoCommandLog] Schema initialized");
        Ok(())
    }
}

#[async_trait]
impl CommandLog for TursoCommandLog {
    async fn record(
        &self,
        command: Operation,
        inverse: Option<Operation>,
        display_name: impl Into<String> + Send,
        entity_type: impl Into<String> + Send,
        entity_id: impl Into<String> + Send,
        target_system: Option<String>,
    ) -> Result<CommandId> {
        // Convert all parameters to owned values before await
        let display_name = display_name.into();
        let entity_type = entity_type.into();
        let entity_id = entity_id.into();

        let id = ulid::Ulid::new().to_string();
        let created_at = chrono::Utc::now().timestamp_millis();

        let operation_json = serde_json::to_string(&command).map_err(|e| {
            StorageError::SerializationError(format!("Failed to serialize operation: {}", e))
        })?;

        let inverse_json = inverse
            .as_ref()
            .map(|inv| {
                serde_json::to_string(inv).map_err(|e| {
                    StorageError::SerializationError(format!("Failed to serialize inverse: {}", e))
                })
            })
            .transpose()?;

        let inverse_value = inverse_json
            .map(|json| turso::Value::Text(json))
            .unwrap_or(turso::Value::Null);
        let target_system_value = target_system
            .map(|sys| turso::Value::Text(sys))
            .unwrap_or(turso::Value::Null);

        let params: Vec<turso::Value> = vec![
            turso::Value::Text(id.clone()),
            turso::Value::Text(operation_json),
            inverse_value,
            turso::Value::Text(display_name),
            turso::Value::Text(entity_type),
            turso::Value::Text(entity_id),
            target_system_value,
            turso::Value::Text(CommandStatus::Pending.as_str().to_string()),
            turso::Value::Text(SyncStatus::Local.as_str().to_string()),
            turso::Value::Integer(created_at),
        ];

        self.db_handle
            .execute(
                "INSERT INTO commands (
                    id, operation, inverse, display_name, entity_type, entity_id,
                    target_system, status, sync_status, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params,
            )
            .await
            .map_err(|e| StorageError::DatabaseError(format!("Failed to insert command: {}", e)))?;

        tracing::debug!("[TursoCommandLog] Recorded command: {}", id);
        Ok(id)
    }

    async fn mark_executed(&self, command_id: &CommandId) -> Result<()> {
        let executed_at = chrono::Utc::now().timestamp_millis();

        self.db_handle
            .execute(
                "UPDATE commands SET status = ?, executed_at = ? WHERE id = ?",
                vec![
                    turso::Value::Text(CommandStatus::Executed.as_str().to_string()),
                    turso::Value::Integer(executed_at),
                    turso::Value::Text(command_id.clone()),
                ],
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to mark command as executed: {}", e))
            })?;

        Ok(())
    }

    async fn mark_undone(&self, command_id: &CommandId, undone_by: &CommandId) -> Result<()> {
        let undone_at = chrono::Utc::now().timestamp_millis();

        self.db_handle
            .execute(
                "UPDATE commands SET status = ?, undone_at = ?, undone_by_command_id = ? WHERE id = ?",
                vec![
                    turso::Value::Text(CommandStatus::Undone.as_str().to_string()),
                    turso::Value::Integer(undone_at),
                    turso::Value::Text(undone_by.clone()),
                    turso::Value::Text(command_id.clone()),
                ],
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to mark command as undone: {}", e))
            })?;

        Ok(())
    }

    async fn mark_redone(&self, command_id: &CommandId) -> Result<()> {
        let executed_at = chrono::Utc::now().timestamp_millis();

        self.db_handle
            .execute(
                "UPDATE commands SET status = ?, executed_at = ?, undone_at = NULL, undone_by_command_id = NULL WHERE id = ?",
                vec![
                    turso::Value::Text(CommandStatus::Executed.as_str().to_string()),
                    turso::Value::Integer(executed_at),
                    turso::Value::Text(command_id.clone()),
                ],
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to mark command as redone: {}", e))
            })?;

        Ok(())
    }

    async fn get_undo_stack(&self, limit: usize) -> Result<Vec<CommandEntry>> {
        let rows = self
            .db_handle
            .query_positional(
                "SELECT id, operation, inverse, display_name, entity_type, entity_id,
                        target_system, status, sync_status, created_at, executed_at,
                        synced_at, undone_at, error_details, undone_by_command_id, undoes_command_id
                 FROM commands
                 WHERE status = 'executed' AND inverse IS NOT NULL
                 ORDER BY created_at DESC
                 LIMIT ?",
                vec![turso::Value::Integer(limit as i64)],
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to query undo stack: {}", e))
            })?;

        let mut commands = Vec::new();
        for row in rows {
            let command = Self::parse_command_entity(&row)?;
            commands.push(command);
        }

        Ok(commands)
    }

    async fn get_redo_stack(&self, limit: usize) -> Result<Vec<CommandEntry>> {
        let rows = self
            .db_handle
            .query_positional(
                "SELECT id, operation, inverse, display_name, entity_type, entity_id,
                        target_system, status, sync_status, created_at, executed_at,
                        synced_at, undone_at, error_details, undone_by_command_id, undoes_command_id
                 FROM commands
                 WHERE status = 'undone' AND inverse IS NOT NULL
                 ORDER BY undone_at DESC
                 LIMIT ?",
                vec![turso::Value::Integer(limit as i64)],
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to query redo stack: {}", e))
            })?;

        let mut commands = Vec::new();
        for row in rows {
            let command = Self::parse_command_entity(&row)?;
            commands.push(command);
        }

        Ok(commands)
    }

    async fn get_command(&self, command_id: &CommandId) -> Result<Option<CommandEntry>> {
        let rows = self
            .db_handle
            .query_positional(
                "SELECT id, operation, inverse, display_name, entity_type, entity_id,
                        target_system, status, sync_status, created_at, executed_at,
                        synced_at, undone_at, error_details, undone_by_command_id, undoes_command_id
                 FROM commands
                 WHERE id = ?",
                vec![turso::Value::Text(command_id.to_string())],
            )
            .await
            .map_err(|e| StorageError::DatabaseError(format!("Failed to query command: {}", e)))?;

        if let Some(row) = rows.into_iter().next() {
            Ok(Some(Self::parse_command_entity(&row)?))
        } else {
            Ok(None)
        }
    }

    async fn update_sync_status(
        &self,
        command_id: &CommandId,
        sync_status: SyncStatus,
    ) -> Result<()> {
        let synced_at = if matches!(sync_status, SyncStatus::Synced) {
            Some(chrono::Utc::now().timestamp_millis())
        } else {
            None
        };

        if let Some(synced_at) = synced_at {
            self.db_handle
                .execute(
                    "UPDATE commands SET sync_status = ?, synced_at = ? WHERE id = ?",
                    vec![
                        turso::Value::Text(sync_status.as_str().to_string()),
                        turso::Value::Integer(synced_at),
                        turso::Value::Text(command_id.clone()),
                    ],
                )
                .await
                .map_err(|e| {
                    StorageError::DatabaseError(format!("Failed to update sync status: {}", e))
                })?;
        } else {
            self.db_handle
                .execute(
                    "UPDATE commands SET sync_status = ? WHERE id = ?",
                    vec![
                        turso::Value::Text(sync_status.as_str().to_string()),
                        turso::Value::Text(command_id.clone()),
                    ],
                )
                .await
                .map_err(|e| {
                    StorageError::DatabaseError(format!("Failed to update sync status: {}", e))
                })?;
        }

        Ok(())
    }

    async fn mark_failed(&self, command_id: &CommandId, error_details: String) -> Result<()> {
        self.db_handle
            .execute(
                "UPDATE commands SET status = ?, error_details = ? WHERE id = ?",
                vec![
                    turso::Value::Text(CommandStatus::Failed.as_str().to_string()),
                    turso::Value::Text(error_details),
                    turso::Value::Text(command_id.clone()),
                ],
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to mark command as failed: {}", e))
            })?;

        Ok(())
    }
}
