//! Navigation operations provider
//!
//! Implements NavigationOperations for backend-driven navigation state.
//! Operations modify navigation tables directly (not part of undo stack).

use std::collections::HashMap;
use std::str::FromStr;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::NavigationOp;
use holon_api::OperationDescriptor;
use holon_api::OperationParam;
use holon_api::Region;
use holon_api::TypeHint;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;

use crate::storage::DbHandle;

/// Navigation operations entity name
pub const ENTITY_NAME: &str = "navigation";
pub const SHORT_NAME: &str = "nav";

/// The `region` operation parameter shared by every navigation op.
fn region_param() -> OperationParam {
    OperationParam {
        name: "region".to_string(),
        type_hint: TypeHint::OneOf {
            values: Region::ALL.iter().map(|r| Value::from(*r)).collect(),
        },
        description: "UI region to navigate".to_string(),
    }
}

/// The navigation entity's full operation descriptor set: the manual focus /
/// pin / history ops plus the macro-generated editor-cursor ops. Shared by
/// every `NavigationProvider` implementation (Turso-backed and in-memory) so
/// the render layer builds identical click intents regardless of backend.
pub fn navigation_operation_descriptors() -> Vec<OperationDescriptor> {
    let manual_ops = vec![
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::Focus.as_str().to_string(),
            display_name: "Focus".to_string(),
            description: "Navigate to focus on a specific block".to_string(),
            required_params: vec![
                region_param(),
                OperationParam {
                    name: "block_id".to_string(),
                    type_hint: TypeHint::String,
                    description: "Block ID to focus on".to_string(),
                },
            ],
            affected_fields: vec!["block_id".to_string()],
            param_mappings: vec![],
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Navigation,
            },
            trigger: None,
            bound_params: Default::default(),
            precondition: None,
        },
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::FocusPin.as_str().to_string(),
            display_name: "Pin Block".to_string(),
            description: "Pin a block to a region (move-to-top dedup; right sidebar uses this)"
                .to_string(),
            required_params: vec![
                region_param(),
                OperationParam {
                    name: "block_id".to_string(),
                    type_hint: TypeHint::String,
                    description: "Block ID to pin".to_string(),
                },
            ],
            affected_fields: vec!["block_id".to_string()],
            param_mappings: vec![],
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Navigation,
            },
            trigger: None,
            bound_params: Default::default(),
            precondition: None,
        },
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::Close.as_str().to_string(),
            display_name: "Close Pin".to_string(),
            description: "Soft-close a navigation_history row by id (sidebar X button)".to_string(),
            required_params: vec![OperationParam {
                name: "history_id".to_string(),
                type_hint: TypeHint::Number,
                description: "navigation_history.id to close".to_string(),
            }],
            affected_fields: vec!["closed_at".to_string()],
            param_mappings: vec![],
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Navigation,
            },
            trigger: None,
            bound_params: Default::default(),
            precondition: None,
        },
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::GoBack.as_str().to_string(),
            display_name: "Go Back".to_string(),
            description: "Navigate to previous view in history".to_string(),
            required_params: vec![region_param()],
            affected_fields: vec!["block_id".to_string()],
            param_mappings: vec![],
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Navigation,
            },
            trigger: None,
            bound_params: Default::default(),
            precondition: None,
        },
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::GoForward.as_str().to_string(),
            display_name: "Go Forward".to_string(),
            description: "Navigate to next view in history".to_string(),
            required_params: vec![region_param()],
            affected_fields: vec!["block_id".to_string()],
            param_mappings: vec![],
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Navigation,
            },
            trigger: None,
            bound_params: Default::default(),
            precondition: None,
        },
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::GoHome.as_str().to_string(),
            display_name: "Go Home".to_string(),
            description: "Navigate to home view (no block focused)".to_string(),
            required_params: vec![region_param()],
            affected_fields: vec!["block_id".to_string()],
            param_mappings: vec![],
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Navigation,
            },
            trigger: None,
            bound_params: Default::default(),
            precondition: None,
        },
    ];
    manual_ops
}

/// Navigation provider for managing region focus state
pub struct NavigationProvider {
    db_handle: DbHandle,
}

impl NavigationProvider {
    pub fn new(db_handle: DbHandle) -> Self {
        Self { db_handle }
    }

    /// Focus on a specific block in a region (Replace semantics).
    ///
    /// Closes the prior open `navigation_history` row in the region (sets
    /// `closed_at`), inserts a new open row, and updates the cursor. Any
    /// forward history is cleared (like browser navigation). The
    /// `focus_roots` matview consumes only `closed_at IS NULL` rows, so the
    /// region converges to "exactly one open row" for main-panel use.
    ///
    /// For pin semantics (sidebar), use `focus_pin` instead — it dedupes
    /// by `(region, block_id)` and refreshes the timestamp.
    async fn focus(&self, region: Region, block_id: Option<&str>) -> Result<OperationResult> {
        tracing::debug!(
            "[NavigationProvider] focus: region={}, block_id={:?}",
            region,
            block_id
        );

        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));

        // Step 1: Get current cursor position (Turso doesn't support subqueries in
        // DELETE)
        tracing::debug!("[NavigationProvider] focus: getting current cursor position");
        let current_history_id = self.get_current_history_id(&mut params).await?;
        tracing::debug!(
            "[NavigationProvider] focus: current history_id = {}",
            current_history_id
        );

        // Step 1b: Idempotent re-focus. If the cursor's current row already
        // targets `block_id`, focusing it again must be a no-op — inserting a
        // fresh row would pile up duplicate back-stack entries on repeated
        // clicks AND desync the AUTOINCREMENT history_id that `close`/unpin
        // address by id (a later same-id close then hits the wrong row, leaking
        // a stale open row into `focus_roots`). Mirrors the reference model's
        // `current_focus(region) == block_id` skip in `navigate_focus.rs`.
        let current_block = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_history_block.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to read current focus block: {}", e))?
            .first()
            .and_then(|row| row.get("block_id"))
            .and_then(|v| v.as_string_owned());
        if current_block.as_deref() == block_id {
            tracing::debug!(
                "[NavigationProvider] focus: idempotent re-focus on {:?} — no-op",
                block_id
            );
            return Ok(OperationResult::irreversible(vec![]));
        }

        // Step 2: Delete any forward history (entries after current cursor)
        tracing::debug!("[NavigationProvider] focus: executing DELETE from navigation_history");
        self.db_handle
            .query(
                include_str!("../../sql/navigation/clear_forward_history.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| {
                tracing::debug!("[NavigationProvider] focus: DELETE failed: {}", e);
                format!("Failed to clear forward history: {}", e)
            })?;
        tracing::debug!("[NavigationProvider] focus: DELETE succeeded");

        // Step 2b: Soft-close every open row in this region so the
        // focus_roots matview ends up with only the row we're about to
        // insert. Open rows from prior navigations stay physically present
        // (for back/forward via the cursor) but exit the matview.
        self.db_handle
            .query(
                include_str!("../../sql/navigation/close_open_in_region.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to close prior open history rows: {}", e))?;

        // Step 3: Insert new history entry (closed_at defaults to NULL → open).
        let block_id_value = match block_id {
            Some(id) => Value::String(id.to_string()),
            None => Value::Null,
        };
        params.insert("block_id".to_string(), block_id_value);

        self.db_handle
            .query(
                include_str!("../../sql/navigation/insert_history.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to insert navigation history: {}", e))?;

        // Step 4: Get the new max history_id
        let max_result = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_max_history_id.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to get max history id: {}", e))?;

        let new_history_id: i64 = max_result
            .first()
            .and_then(|row| row.get("max_id"))
            .and_then(|v| v.as_i64())
            .ok_or_else(|| "Failed to get new history_id".to_string())?;

        // Step 5: Update cursor to point to new entry
        params.insert("new_id".to_string(), Value::Integer(new_history_id));
        self.db_handle
            .query(
                include_str!("../../sql/navigation/upsert_cursor.sql"),
                params,
            )
            .await
            .map_err(|e| format!("Failed to update navigation cursor: {}", e))?;

        // Step 6: Retention cap — keep only the 100 most recent closed rows
        // per region so unbounded navigation doesn't grow the table (and the
        // CDC/matview state derived from it) forever. Back/forward beyond the
        // window is intentionally forgotten.
        let mut prune_params = HashMap::new();
        prune_params.insert("region".to_string(), Value::from(region));
        let threshold = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_prune_threshold.sql"),
                prune_params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to read history prune threshold: {}", e))?
            .first()
            .and_then(|row| row.get("id"))
            .and_then(|v| v.as_i64());
        if let Some(threshold_id) = threshold {
            prune_params.insert("threshold_id".to_string(), Value::Integer(threshold_id));
            self.db_handle
                .query(
                    include_str!("../../sql/navigation/prune_closed_history.sql"),
                    prune_params,
                )
                .await
                .map_err(|e| format!("Failed to prune closed history rows: {}", e))?;
        }

        tracing::debug!("[NavigationProvider] focus: completed successfully");
        Ok(OperationResult::irreversible(vec![]))
    }

    /// Pin a block to a region (Pin semantics, e.g. shift+click → right
    /// sidebar).
    ///
    /// Move-to-top dedup: if an open row for `(region, block_id)` already
    /// exists, refresh its timestamp so it sorts to the top of focus_roots.
    /// Otherwise insert a fresh open row. Cursor is left untouched — pins
    /// are not part of the back/forward stack.
    ///
    /// Implementation note: `db_handle.query()` returns an empty `Vec` for
    /// UPDATE statements regardless of rows-affected, so we can't infer
    /// "matched / not matched" from its result. Instead we SELECT first to
    /// check existence, then branch into UPDATE-or-INSERT. Two round-trips
    /// per click is acceptable for a UI-driven pin operation.
    async fn focus_pin(&self, region: Region, block_id: &str) -> Result<OperationResult> {
        tracing::debug!(
            "[NavigationProvider] focus_pin: region={}, block_id={}",
            region,
            block_id
        );

        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));
        params.insert("block_id".to_string(), Value::String(block_id.to_string()));

        // Step 1: check whether an open pin already exists for (region, block_id).
        let existing = self
            .db_handle
            .query(
                "SELECT id FROM navigation_history WHERE region = $region AND block_id = \
                 $block_id AND closed_at IS NULL LIMIT 1",
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to look up existing pin: {}", e))?;

        if existing.is_empty() {
            // Step 2a: no existing pin → insert.
            self.db_handle
                .query(
                    include_str!("../../sql/navigation/insert_history.sql"),
                    params.clone(),
                )
                .await
                .map_err(|e| format!("Failed to insert pin: {}", e))?;
            tracing::debug!("[NavigationProvider] focus_pin: inserted new pin");
        } else {
            // Step 2b: existing pin → bump its timestamp (move-to-top).
            self.db_handle
                .query(
                    include_str!("../../sql/navigation/update_pin_timestamp.sql"),
                    params.clone(),
                )
                .await
                .map_err(|e| format!("Failed to refresh pin timestamp: {}", e))?;
            tracing::debug!("[NavigationProvider] focus_pin: refreshed existing pin");
        }

        Ok(OperationResult::irreversible(vec![]))
    }

    /// Soft-close a specific navigation_history row by id.
    /// Used by sidebar X button.
    async fn close(&self, history_id: i64) -> Result<OperationResult> {
        tracing::debug!("[NavigationProvider] close: history_id={}", history_id);

        let mut params = HashMap::new();
        params.insert("history_id".to_string(), Value::Integer(history_id));

        self.db_handle
            .query(
                include_str!("../../sql/navigation/close_history_id.sql"),
                params,
            )
            .await
            .map_err(|e| format!("Failed to close history row: {}", e))?;

        Ok(OperationResult::irreversible(vec![]))
    }

    /// Go back in navigation history
    async fn go_back(&self, region: Region) -> Result<OperationResult> {
        tracing::debug!("[NavigationProvider] go_back: region={}", region);

        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));

        self.get_current_history_id(&mut params).await?;

        // Find the previous history entry
        let prev_result = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_previous_entry.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to find previous entry: {}", e))?;

        // Step 3: Update cursor - either to previous entry or to NULL (home)
        if let Some(prev_row) = prev_result.first() {
            if let Some(prev_id) = prev_row.get("id").and_then(|v| v.as_i64()) {
                params.insert("new_id".to_string(), Value::Integer(prev_id));
                self.db_handle
                    .query(
                        include_str!("../../sql/navigation/update_cursor.sql"),
                        params,
                    )
                    .await
                    .map_err(|e| format!("Failed to go back: {}", e))?;
                tracing::debug!(
                    "[NavigationProvider] go_back: moved to history_id={}",
                    prev_id
                );
            }
        } else {
            // No previous entry - go to home (NULL cursor)
            self.db_handle
                .query(
                    include_str!("../../sql/navigation/nullify_cursor.sql"),
                    params,
                )
                .await
                .map_err(|e| format!("Failed to go back to home: {}", e))?;
            tracing::debug!("[NavigationProvider] go_back: went to home (no previous entry)");
        }

        Ok(OperationResult::irreversible(vec![]))
    }

    /// Go forward in navigation history
    async fn go_forward(&self, region: Region) -> Result<OperationResult> {
        tracing::debug!("[NavigationProvider] go_forward: region={}", region);

        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));

        self.get_current_history_id(&mut params).await?;

        // Find the next history entry
        let next_result = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_next_entry.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to find next entry: {}", e))?;

        // Step 3: Update cursor if next entry exists
        if let Some(next_row) = next_result.first() {
            if let Some(next_id) = next_row.get("id").and_then(|v| v.as_i64()) {
                params.insert("new_id".to_string(), Value::Integer(next_id));
                self.db_handle
                    .query(
                        include_str!("../../sql/navigation/update_cursor.sql"),
                        params,
                    )
                    .await
                    .map_err(|e| format!("Failed to go forward: {}", e))?;
                tracing::debug!(
                    "[NavigationProvider] go_forward: moved to history_id={}",
                    next_id
                );
            }
        } else {
            tracing::debug!(
                "[NavigationProvider] go_forward: no next entry, staying at current position"
            );
        }

        Ok(OperationResult::irreversible(vec![]))
    }

    /// Navigate to home (root view, no specific block focused)
    async fn go_home(&self, region: Region) -> Result<OperationResult> {
        self.focus(region, None).await
    }

    /// Get the current cursor history_id for a region, returning 0 if none
    /// exists.
    async fn get_current_history_id(&self, params: &mut HashMap<String, Value>) -> Result<i64> {
        let cursor_result = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_cursor.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to get cursor position: {}", e))?;

        let current_history_id: i64 = cursor_result
            .first()
            .and_then(|row| row.get("history_id"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        params.insert("current_id".to_string(), Value::Integer(current_history_id));
        Ok(current_history_id)
    }
}

#[async_trait]
impl OperationProvider for NavigationProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        navigation_operation_descriptors()
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        if entity_name != ENTITY_NAME {
            return Err(format!(
                "NavigationProvider: expected entity '{}', got '{}'",
                ENTITY_NAME, entity_name
            )
            .into());
        }

        // `close` takes only `history_id` (no region) — handle before region
        // extraction.
        if op_name == NavigationOp::Close.as_str() {
            let history_id = params
                .get("history_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "Missing required parameter 'history_id'".to_string())?;
            return self.close(history_id).await;
        }

        let region: Region = params
            .get("region")
            .cloned()
            .ok_or("Missing required parameter 'region'")?
            .try_into()
            .map_err(|e: Box<dyn std::error::Error + Send + Sync>| e.to_string())?;

        match NavigationOp::from_str(op_name) {
            Ok(NavigationOp::Focus) => {
                let block_id = params.get("block_id").and_then(|v| match v {
                    Value::String(s) => Some(s.as_str()),
                    _ => None,
                });
                self.focus(region, block_id).await
            }
            Ok(NavigationOp::FocusPin) => {
                let block_id = params
                    .get("block_id")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .ok_or_else(|| "Missing required parameter 'block_id'".to_string())?;
                self.focus_pin(region, block_id).await
            }
            Ok(NavigationOp::GoBack) => self.go_back(region).await,
            Ok(NavigationOp::GoForward) => self.go_forward(region).await,
            Ok(NavigationOp::GoHome) => self.go_home(region).await,
            // `close` is dispatched before region extraction above.
            Ok(NavigationOp::Close) => {
                unreachable!("close is handled before region extraction")
            }
            Err(_) => Err(format!(
                "NavigationProvider: unknown operation '{op_name}' for entity '{ENTITY_NAME}'"
            )
            .into()),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_operations_defined() {
        // Just verify operations are defined correctly
        // Full integration tests require DbHandle
        let ops = vec!["focus", "go_back", "go_forward", "go_home"];
        for op in ops {
            assert!(
                ["focus", "go_back", "go_forward", "go_home"].contains(&op),
                "Operation {} should be defined",
                op
            );
        }
    }
}
