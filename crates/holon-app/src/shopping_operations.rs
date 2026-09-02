//! The shopping list's one bespoke operation: `shopping_sync`.
//!
//! It performs ONE round — pull, reconcile, push — and hands the local writes
//! back as follow-up operations, so every `shopping_item` row is written by the
//! declared type's own generic authority and the sync adds no second writer.
//!
//! No cadence lives here. The operation runs when something calls it.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use holon::storage::DbHandle;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result as DatasourceResult;
use holon_core::storage::types::StorageEntity;
use holon_kitchen::shopping::ItemKey;
use holon_kitchen::shopping::LocalShoppingItem;
use holon_kitchen::shopping::ShoppingCategory;
use holon_kitchen::shopping::ShoppingReconciler;
use holon_kitchen::shopping_sync::ShoppingRowReader;
use holon_kitchen::shopping_sync::local_intent_operation;
use holon_kitchen::shopping_sync::sync_once;

use crate::mcp_integrations::McpIntegrationRegistry;
use crate::shopping_rest::RestShoppingPeer;

const ENTITY: &str = "shopping_item";
const TABLE: &str = "shopping_item_raw";
/// The sidecar this operation drives, by provider name.
pub const PROVIDER: &str = "shopping";

/// What Holon calls itself on the peer's commits. The peer uses it for its own
/// bookkeeping only; two Holon installs sharing one list are indistinguishable
/// to it, which costs nothing while the commit protocol carries no per-device
/// state.
pub fn device_id() -> &'static str {
    "holon"
}

pub struct ShoppingOperations {
    registry: Arc<McpIntegrationRegistry>,
    db_handle: DbHandle,
    /// Stable per install, echoed on every commit. Not a secret — the
    /// credential is the URL.
    device_id: String,
}

impl ShoppingOperations {
    pub fn new(
        registry: Arc<McpIntegrationRegistry>,
        db_handle: DbHandle,
        device_id: impl Into<String>,
    ) -> Self {
        Self {
            registry,
            db_handle,
            device_id: device_id.into(),
        }
    }

    fn peer(&self) -> DatasourceResult<RestShoppingPeer> {
        let integration = self.registry.by_name(PROVIDER).ok_or_else(|| {
            format!(
                "shopping_sync: the '{PROVIDER}' integration is not connected, so there is \
                 nothing to sync with. Its boot outcome was disclosed on the degraded bus — a \
                 missing SHOPPING_LIST_URL is the usual cause."
            )
        })?;
        Ok(RestShoppingPeer::new(
            integration.sync_engine.call_surface(),
            self.device_id.clone(),
        ))
    }
}

/// Reads the local rows the reconciler decides against. Read-only by design:
/// the writes go back through the dispatcher as follow-ups.
struct Rows {
    db_handle: DbHandle,
}

#[async_trait]
impl ShoppingRowReader for Rows {
    async fn load(&self) -> anyhow::Result<Vec<LocalShoppingItem>> {
        let sql = format!(
            "SELECT id, name, cat, count, checked, product_id, deleted_at, last_seen_remote FROM \
             {TABLE}"
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("shopping_sync: reading the local list: {e}"))?;
        rows.iter().map(row_to_item).collect()
    }
}

fn row_to_item(row: &HashMap<Arc<str>, Value>) -> anyhow::Result<LocalShoppingItem> {
    let text = |column: &str| -> anyhow::Result<String> {
        row.get(column)
            .and_then(Value::as_string)
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow::anyhow!("shopping_sync: `{TABLE}.{column}` is missing or not text")
            })
    };
    let optional_text = |column: &str| -> Option<String> {
        row.get(column)
            .and_then(Value::as_string)
            .map(str::to_string)
    };
    let category = ShoppingCategory::unresolved(&text("cat")?);
    let name = text("name")?;
    let id = text("id").unwrap_or_else(|_| ItemKey::new(name.clone(), &category).row_id());
    Ok(LocalShoppingItem {
        id,
        name,
        category,
        count: row.get("count").and_then(number),
        checked: row.get("checked").and_then(number).unwrap_or(0.0) != 0.0,
        product_id: optional_text("product_id"),
        deleted_at: optional_text("deleted_at"),
        last_seen_remote: optional_text("last_seen_remote"),
    })
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(f) => Some(*f),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

#[async_trait]
impl OperationProvider for ShoppingOperations {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![OperationDescriptor {
            entity_name: EntityName::new(ENTITY),
            entity_short_name: ENTITY.to_string(),
            name: "shopping_sync".to_string(),
            display_name: "Sync shopping list".to_string(),
            description: "Exchange one round of changes with the shopping-list peer".to_string(),
            // No list parameter: the sidecar's `base_url` IS the share link of
            // one list, so the connector already names what it syncs.
            required_params: vec![],
            id_column: "id".to_string(),
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Internal,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        }]
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        _: StorageEntity,
    ) -> DatasourceResult<OperationResult> {
        // Compare through `EntityName`, which normalizes `shopping_item` to its
        // canonical `shopping-item`: the dispatcher routes by the normalized
        // name, so a raw `&str` compare here rejects every real dispatch.
        if *entity_name != EntityName::new(ENTITY) || op_name != "shopping_sync" {
            return Err(format!(
                "ShoppingOperations serves only {ENTITY}/shopping_sync, not \
                 {entity_name}/{op_name}"
            )
            .into());
        }

        let peer = self.peer()?;
        let rows = Rows {
            db_handle: self.db_handle.clone(),
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("shopping_sync: the system clock is before the epoch: {e}"))?
            .as_millis() as i64;
        let declaration = holon_kitchen::shopping_item_type()
            .map_err(|e| format!("shopping_sync: {e:#}"))?
            .soft_delete
            .ok_or_else(|| {
                "shopping_sync: `shopping_item` declares no soft deletion, so a local delete \
                 leaves no tombstone for this round to push"
                    .to_string()
            })?;
        let outcome = sync_once(
            &peer,
            &rows,
            &ShoppingReconciler::with_tombstone_window(declaration.retention()),
            &self.device_id,
            now_ms,
        )
        .await
        .map_err(|e| format!("shopping_sync: {e:#}"))?;

        let follow_ups = outcome.local.iter().map(local_intent_operation).collect();
        // A completed exchange with another peer has no inverse: undoing it
        // would push the reverse commands at a list that has already moved on.
        Ok(
            OperationResult::declared_irreversible(Vec::new(), "a peer sync cannot be un-sent")
                .with_follow_ups(follow_ups),
        )
    }
}
