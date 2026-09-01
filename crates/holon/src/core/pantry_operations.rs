//! The pantry's one bespoke operation: `consume`.
//!
//! `add` and `adjust` are already the declared type's generic `create` and
//! `set_field` — adding kitchen-named aliases for them would be a second door
//! onto the same write with nothing behind it. `consume` earns its own
//! operation because it is a read-modify-write with two refusals the generic
//! `set_field` cannot make: consuming past empty, and consuming in a unit we
//! have no factor for.

use std::collections::HashMap;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::Operation;
use holon_api::OperationDescriptor;
use holon_api::OperationParam;
use holon_api::TypeHint;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result as DatasourceResult;
use holon_core::storage::types::StorageEntity;

use crate::storage::turso::DbHandle;

const ENTITY: &str = "pantry_item";
const TABLE: &str = "pantry_item_raw";

pub struct PantryOperations {
    db_handle: DbHandle,
}

impl PantryOperations {
    pub fn new(db_handle: DbHandle) -> Self {
        Self { db_handle }
    }

    async fn read_stock(&self, id: &str) -> DatasourceResult<(f64, Option<String>)> {
        let sql = format!(
            "SELECT quantity, unit FROM {TABLE} WHERE id = '{}'",
            id.replace('\'', "''")
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("consume: reading pantry item '{id}': {e}"))?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| format!("consume: no pantry item with id '{id}'"))?;
        let quantity = row
            .get("quantity")
            .and_then(value_as_f64)
            .ok_or_else(|| format!("consume: pantry item '{id}' has no numeric quantity"))?;
        let unit = row
            .get("unit")
            .and_then(|v| v.as_string())
            .map(str::to_string);
        Ok((quantity, unit))
    }
}

fn value_as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Float(f) => Some(*f),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

fn unit_label(unit: Option<&str>) -> String {
    unit.map(|u| u.to_string())
        .unwrap_or_else(|| "(no unit)".to_string())
}

#[async_trait]
impl OperationProvider for PantryOperations {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![OperationDescriptor {
            entity_name: EntityName::new(ENTITY),
            entity_short_name: ENTITY.to_string(),
            name: "consume".to_string(),
            display_name: "Consume".to_string(),
            description: "Use up some of a pantry item".to_string(),
            required_params: vec![
                OperationParam {
                    name: "id".to_string(),
                    type_hint: TypeHint::String,
                    description: "Pantry item ID".to_string(),
                },
                OperationParam {
                    name: "quantity".to_string(),
                    type_hint: TypeHint::Number,
                    description: "How much to use up".to_string(),
                },
                OperationParam {
                    name: "unit".to_string(),
                    type_hint: TypeHint::String,
                    description: "Unit the amount is expressed in".to_string(),
                },
            ],
            id_column: "id".to_string(),
            affected_fields: vec!["quantity".to_string()],
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
        params: StorageEntity,
    ) -> DatasourceResult<OperationResult> {
        // Compare through `EntityName`, which normalizes `pantry_item` to its
        // canonical `pantry-item`: the dispatcher routes by the normalized
        // name, so a raw `&str` compare here rejects every real dispatch.
        if *entity_name != EntityName::new(ENTITY) || op_name != "consume" {
            return Err(format!(
                "PantryOperations serves only {ENTITY}/consume, not {entity_name}/{op_name}"
            )
            .into());
        }

        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or("consume: missing 'id' parameter")?
            .to_string();
        let asked = params
            .get("quantity")
            .and_then(value_as_f64)
            .ok_or("consume: missing numeric 'quantity' parameter")?;
        let asked_unit = params
            .get("unit")
            .and_then(|v| v.as_string())
            .map(str::to_string);

        let (on_hand, stocked_unit) = self.read_stock(&id).await?;

        // Same-unit only: the conversion factors live on `product`, an Inc D
        // type. Scaling by a guessed factor would silently empty a pantry.
        if stocked_unit.as_deref() != asked_unit.as_deref() {
            return Err(format!(
                "consume: pantry item '{id}' is stocked in {} but {} was asked for, and no \
                 conversion factor exists between them (unit conversion arrives with the product \
                 nutrition table). Consume in {} instead.",
                unit_label(stocked_unit.as_deref()),
                unit_label(asked_unit.as_deref()),
                unit_label(stocked_unit.as_deref()),
            )
            .into());
        }

        if asked > on_hand {
            return Err(format!(
                "consume: pantry item '{id}' holds {on_hand} {unit} but {asked} {unit} was asked \
                 for. A pantry cannot go negative — adjust the stocked amount if it is wrong.",
                unit = unit_label(stocked_unit.as_deref()),
            )
            .into());
        }

        let remaining = on_hand - asked;
        let sql = format!(
            "UPDATE {TABLE} SET quantity = {remaining} WHERE id = '{}'",
            id.replace('\'', "''")
        );
        self.db_handle
            .execute(&sql, vec![])
            .await
            .map_err(|e| format!("consume: updating pantry item '{id}': {e}"))?;

        // The inverse restores the exact prior amount rather than re-adding
        // `asked`: an interleaved adjust must not be undone away by arithmetic.
        let mut undo_params: HashMap<String, Value> = HashMap::new();
        undo_params.insert("id".to_string(), Value::String(id));
        undo_params.insert("field".to_string(), Value::String("quantity".to_string()));
        undo_params.insert("value".to_string(), Value::Float(on_hand));

        Ok(OperationResult::new(
            Vec::new(),
            Operation::new(ENTITY, "set_field", "Restock", undo_params),
        ))
    }
}
