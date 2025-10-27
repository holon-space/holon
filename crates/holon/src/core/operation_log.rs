//! Operation log implementation for persistent undo/redo.
//!
//! This module provides `OperationLogStore`, which implements the
//! `OperationLogOperations` trait for persistent operation logging.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info};

use crate::storage::DbHandle;
use holon_api::{HasSchema, Operation, Value};
use holon_core::{OperationLogEntry, OperationLogOperations, OperationStatus, UndoAction};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Persistent operation log store backed by DbHandle.
///
/// Stores operations in the `operations` table and provides
/// undo/redo candidate queries.
pub struct OperationLogStore {
    db_handle: DbHandle,
    max_log_size: usize,
}

impl OperationLogStore {
    /// Create a new operation log store.
    pub fn new(db_handle: DbHandle) -> Self {
        Self {
            db_handle,
            max_log_size: 100,
        }
    }

    /// Create a new operation log store with custom max size.
    pub fn with_max_size(db_handle: DbHandle, max_log_size: usize) -> Self {
        Self {
            db_handle,
            max_log_size,
        }
    }

    /// Initialize the operations table schema.
    pub async fn initialize_schema(&self) -> Result<()> {
        let schema = OperationLogEntry::schema();
        let create_table_sql = schema.to_create_table_sql();
        let index_sqls = schema.to_index_sql();

        debug!("Creating operations table: {}", create_table_sql);
        self.db_handle
            .execute_ddl(&create_table_sql)
            .await
            .map_err(|e| format!("Failed to create operations table: {}", e))?;

        for index_sql in index_sqls {
            debug!("Creating index: {}", index_sql);
            self.db_handle
                .execute_ddl(&index_sql)
                .await
                .map_err(|e| format!("Failed to create index: {}", e))?;
        }

        info!("Operation log schema initialized");
        Ok(())
    }

    /// Trim old operations if we're over the max size.
    async fn trim_if_needed(&self) -> Result<()> {
        let count_result = self
            .db_handle
            .query("SELECT COUNT(*) as count FROM operation", HashMap::new())
            .await
            .map_err(|e| format!("Failed to count operations: {}", e))?;

        let count = count_result
            .first()
            .and_then(|row| row.get("count"))
            .and_then(|v| v.as_i64())
            .expect("COUNT(*) query must return a numeric result") as usize;

        if count > self.max_log_size {
            let to_delete = count - self.max_log_size;
            debug!(
                "Trimming {} old operations (current: {}, max: {})",
                to_delete, count, self.max_log_size
            );

            // Delete oldest entries (lowest IDs)
            let delete_sql = format!(
                "DELETE FROM operation WHERE id IN (
                    SELECT id FROM operation ORDER BY id ASC LIMIT {}
                )",
                to_delete
            );

            self.db_handle
                .query(&delete_sql, HashMap::new())
                .await
                .map_err(|e| format!("Failed to trim old operations: {}", e))?;
        }

        Ok(())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl OperationLogOperations for OperationLogStore {
    async fn log_operation(&self, operation: Operation, inverse: UndoAction) -> Result<i64> {
        // Clear redo stack first (new operation invalidates redo history)
        self.clear_redo_stack().await?;

        // Create the entry
        let entry = OperationLogEntry::new(operation, inverse.into_option());

        let insert_sql = "INSERT INTO operation (operation, inverse, status, created_at, display_name, entity_name, op_name)
                          VALUES ($operation, $inverse, $status, $created_at, $display_name, $entity_name, $op_name)";

        let mut params = HashMap::new();
        params.insert(
            "operation".to_string(),
            Value::String(entry.operation.clone()),
        );
        params.insert(
            "inverse".to_string(),
            entry
                .inverse
                .as_ref()
                .map(|s| Value::String(s.clone()))
                .unwrap_or(Value::Null),
        );
        params.insert("status".to_string(), Value::String(entry.status.clone()));
        params.insert("created_at".to_string(), Value::Integer(entry.created_at));
        params.insert(
            "display_name".to_string(),
            Value::String(entry.display_name.clone()),
        );
        params.insert(
            "entity_name".to_string(),
            Value::String(entry.entity_name.clone()),
        );
        params.insert("op_name".to_string(), Value::String(entry.op_name.clone()));

        self.db_handle
            .query(insert_sql, params)
            .await
            .map_err(|e| format!("Failed to insert operation log entry: {}", e))?;

        // Get the inserted ID
        let id_result = self
            .db_handle
            .query("SELECT last_insert_rowid() as id", HashMap::new())
            .await
            .map_err(|e| format!("Failed to get last insert ID: {}", e))?;

        let id = id_result
            .first()
            .and_then(|row| row.get("id"))
            .and_then(|v| v.as_i64())
            .ok_or("Failed to get inserted operation ID")?;

        // Trim if needed
        self.trim_if_needed().await?;

        debug!("Logged operation {} with id {}", entry.display_name, id);
        Ok(id)
    }

    async fn mark_undone(&self, id: i64) -> Result<()> {
        let sql = "UPDATE operation SET status = $status WHERE id = $id";
        let mut params = HashMap::new();
        params.insert(
            "status".to_string(),
            Value::String(OperationStatus::Undone.as_str().to_string()),
        );
        params.insert("id".to_string(), Value::Integer(id));

        self.db_handle
            .query(sql, params)
            .await
            .map_err(|e| format!("Failed to mark operation as undone: {}", e))?;

        debug!("Marked operation {} as undone", id);
        Ok(())
    }

    async fn mark_redone(&self, id: i64) -> Result<()> {
        let sql = "UPDATE operation SET status = $status WHERE id = $id";
        let mut params = HashMap::new();
        params.insert(
            "status".to_string(),
            Value::String(OperationStatus::PendingSync.as_str().to_string()),
        );
        params.insert("id".to_string(), Value::Integer(id));

        self.db_handle
            .query(sql, params)
            .await
            .map_err(|e| format!("Failed to mark operation as redone: {}", e))?;

        debug!("Marked operation {} as redone", id);
        Ok(())
    }

    async fn clear_redo_stack(&self) -> Result<()> {
        let sql = "UPDATE operation SET status = $new_status WHERE status = $old_status";
        let mut params = HashMap::new();
        params.insert(
            "new_status".to_string(),
            Value::String(OperationStatus::Cancelled.as_str().to_string()),
        );
        params.insert(
            "old_status".to_string(),
            Value::String(OperationStatus::Undone.as_str().to_string()),
        );

        self.db_handle
            .query(sql, params)
            .await
            .map_err(|e| format!("Failed to clear redo stack: {}", e))?;

        debug!("Cleared redo stack");
        Ok(())
    }

    fn max_log_size(&self) -> usize {
        self.max_log_size
    }
}

/// Observer that logs operations to the persistent OperationLogStore.
///
/// This observer implements OperationObserver and delegates to OperationLogStore.
/// It observes all operations (entity_filter = "*") and logs them for undo/redo.
pub struct OperationLogObserver {
    store: Arc<OperationLogStore>,
}

impl OperationLogObserver {
    /// Create a new operation log observer wrapping the given store.
    pub fn new(store: Arc<OperationLogStore>) -> Self {
        Self { store }
    }
}

use crate::core::datasource::OperationObserver;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl OperationObserver for OperationLogObserver {
    fn entity_filter(&self) -> &str {
        "*" // Observe all entities for undo/redo
    }

    async fn on_operation_executed(
        &self,
        operation: &holon_api::Operation,
        undo_action: &UndoAction,
    ) {
        if let Err(e) = self
            .store
            .log_operation(operation.clone(), undo_action.clone())
            .await
        {
            tracing::error!("Failed to log operation for undo: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::turso::TursoBackend;

    async fn make_db_handle() -> DbHandle {
        let (_backend, db_handle) = TursoBackend::new_in_memory()
            .await
            .expect("Failed to create backend");
        db_handle
    }

    #[tokio::test]
    async fn test_operation_log_store_basic() {
        let db_handle = make_db_handle().await;
        let store = OperationLogStore::new(db_handle.clone());
        store
            .initialize_schema()
            .await
            .expect("Failed to initialize schema");

        let op = Operation::new(
            "test-entity",
            "test_op",
            "Test Operation",
            HashMap::from([("id".to_string(), Value::String("123".to_string()))]),
        );
        let inverse = Operation::new(
            "test-entity",
            "test_op_inverse",
            "Undo Test Operation",
            HashMap::from([("id".to_string(), Value::String("123".to_string()))]),
        );

        let id = store
            .log_operation(op, UndoAction::Undo(inverse))
            .await
            .expect("Failed to log operation");
        assert!(id > 0);

        let result = db_handle
            .query(
                "SELECT * FROM operation WHERE id = $id",
                HashMap::from([("id".to_string(), Value::Integer(id))]),
            )
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].get("display_name").and_then(|v| v.as_string()),
            Some("Test Operation")
        );
        assert_eq!(
            result[0].get("status").and_then(|v| v.as_string()),
            Some("pending_sync")
        );
    }

    #[tokio::test]
    async fn test_mark_undone_and_redone() {
        let db_handle = make_db_handle().await;
        let store = OperationLogStore::new(db_handle.clone());
        store
            .initialize_schema()
            .await
            .expect("Failed to initialize schema");

        let op = Operation::new("test", "op1", "Op 1", HashMap::new());
        let id = store
            .log_operation(op, UndoAction::Irreversible)
            .await
            .unwrap();

        store.mark_undone(id).await.unwrap();

        let result = db_handle
            .query(
                "SELECT status FROM operation WHERE id = $id",
                HashMap::from([("id".to_string(), Value::Integer(id))]),
            )
            .await
            .unwrap();
        assert_eq!(
            result[0].get("status").and_then(|v| v.as_string()),
            Some("undone")
        );

        store.mark_redone(id).await.unwrap();

        let result = db_handle
            .query(
                "SELECT status FROM operation WHERE id = $id",
                HashMap::from([("id".to_string(), Value::Integer(id))]),
            )
            .await
            .unwrap();
        assert_eq!(
            result[0].get("status").and_then(|v| v.as_string()),
            Some("pending_sync")
        );
    }

    #[tokio::test]
    async fn test_clear_redo_stack_on_new_operation() {
        let db_handle = make_db_handle().await;
        let store = OperationLogStore::new(db_handle.clone());
        store
            .initialize_schema()
            .await
            .expect("Failed to initialize schema");

        let op1 = Operation::new("test", "op1", "Op 1", HashMap::new());
        let id1 = store
            .log_operation(op1, UndoAction::Irreversible)
            .await
            .unwrap();

        store.mark_undone(id1).await.unwrap();

        let op2 = Operation::new("test", "op2", "Op 2", HashMap::new());
        store
            .log_operation(op2, UndoAction::Irreversible)
            .await
            .unwrap();

        let result = db_handle
            .query(
                "SELECT status FROM operation WHERE id = $id",
                HashMap::from([("id".to_string(), Value::Integer(id1))]),
            )
            .await
            .unwrap();
        assert_eq!(
            result[0].get("status").and_then(|v| v.as_string()),
            Some("cancelled")
        );
    }

    #[tokio::test]
    async fn test_trim_old_operations() {
        let db_handle = make_db_handle().await;
        let store = OperationLogStore::with_max_size(db_handle.clone(), 5);
        store
            .initialize_schema()
            .await
            .expect("Failed to initialize schema");

        for i in 0..10 {
            let op = Operation::new(
                "test",
                &format!("op{}", i),
                &format!("Op {}", i),
                HashMap::new(),
            );
            store
                .log_operation(op, UndoAction::Irreversible)
                .await
                .unwrap();
        }

        let count_result = db_handle
            .query("SELECT COUNT(*) as count FROM operation", HashMap::new())
            .await
            .unwrap();
        let count = count_result
            .first()
            .and_then(|row| row.get("count"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn test_operations_survive_new_store_instance() {
        let db_handle = make_db_handle().await;

        {
            let store = OperationLogStore::new(db_handle.clone());
            store.initialize_schema().await.unwrap();

            let op = Operation::new("test", "op1", "Op 1", HashMap::new());
            let inverse = Operation::new("test", "op1_inv", "Undo Op 1", HashMap::new());
            store
                .log_operation(op, UndoAction::Undo(inverse))
                .await
                .unwrap();

            let op2 = Operation::new("test", "op2", "Op 2", HashMap::new());
            store
                .log_operation(op2, UndoAction::Irreversible)
                .await
                .unwrap();
        }

        let store2 = OperationLogStore::new(db_handle.clone());
        store2.initialize_schema().await.unwrap();

        let result = db_handle
            .query("SELECT COUNT(*) as count FROM operation", HashMap::new())
            .await
            .unwrap();
        let count = result
            .first()
            .and_then(|row| row.get("count"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        assert_eq!(count, 2, "Operations should persist across store instances");

        let ops = db_handle
            .query(
                "SELECT inverse FROM operation ORDER BY id ASC LIMIT 1",
                HashMap::new(),
            )
            .await
            .unwrap();
        let inverse_val = ops[0].get("inverse");
        assert!(
            inverse_val.is_some() && !matches!(inverse_val, Some(Value::Null)),
            "Inverse operation should persist across store instances, got: {:?}",
            inverse_val
        );
    }

    #[tokio::test]
    async fn test_trim_does_not_remove_undone_operations() {
        let db_handle = make_db_handle().await;
        let store = OperationLogStore::with_max_size(db_handle.clone(), 3);
        store.initialize_schema().await.unwrap();

        let mut ids = Vec::new();
        for i in 0..3 {
            let op = Operation::new(
                "test",
                &format!("op{}", i),
                &format!("Op {}", i),
                HashMap::new(),
            );
            let inverse = Operation::new(
                "test",
                &format!("op{}_inv", i),
                &format!("Undo Op {}", i),
                HashMap::new(),
            );
            let id = store
                .log_operation(op, UndoAction::Undo(inverse))
                .await
                .unwrap();
            ids.push(id);
        }

        store.mark_undone(ids[2]).await.unwrap();

        let op = Operation::new("test", "op_new", "New Op", HashMap::new());
        store
            .log_operation(op, UndoAction::Irreversible)
            .await
            .unwrap();

        let result = db_handle
            .query(
                "SELECT status FROM operation WHERE id = $id",
                HashMap::from([("id".to_string(), Value::Integer(ids[2]))]),
            )
            .await
            .unwrap();
        assert_eq!(
            result[0].get("status").and_then(|v| v.as_string()),
            Some("cancelled"),
            "Undone operation should be cancelled after new operation"
        );
    }

    #[tokio::test]
    async fn test_concurrent_log_operations_all_persist() {
        let db_handle = make_db_handle().await;
        let store = Arc::new(OperationLogStore::new(db_handle.clone()));
        store.initialize_schema().await.unwrap();

        let mut handles = Vec::new();
        for i in 0..10 {
            let store_clone = store.clone();
            handles.push(tokio::spawn(async move {
                let op = Operation::new(
                    "test",
                    &format!("op{}", i),
                    &format!("Op {}", i),
                    HashMap::new(),
                );
                store_clone
                    .log_operation(op, UndoAction::Irreversible)
                    .await
                    .unwrap()
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let result = db_handle
            .query("SELECT COUNT(*) as count FROM operation", HashMap::new())
            .await
            .unwrap();
        let count = result
            .first()
            .and_then(|row| row.get("count"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        assert_eq!(count, 10, "All concurrent operations should persist");
    }
}
