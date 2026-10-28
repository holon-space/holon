//! The write rule an entity's authority derives from its schema: which params
//! are columns, which go to the overflow bag, which are refused, and the CRUD
//! descriptors it advertises.
//!
//! One rule for every store that serves a typed entity, so the SQL and the
//! Loro authority cannot drift apart in what they accept.

use std::collections::HashMap;
use std::collections::HashSet;

use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::OperationParam;
use holon_api::TargetScope;
use holon_api::TypeDefinition;
use holon_api::TypeHint;
use holon_api::Value;
use holon_api::arcs::TransitionArcs;
use holon_api::marking::MarkingDelta;

use crate::Result;
use crate::block_ordering::is_operation_control_param;
use crate::storage::types::StorageEntity;

/// The column vocabulary one entity's write table exposes, derived from that
/// entity's schema.
///
/// Everything the write path used to ASSUME about `block` — that a param
/// outside the column set can be packed into a `properties` JSON column, that
/// a create stamps `created_at`/`updated_at` — is a question asked of this
/// schema instead. A runtime-declared type that declares none of those columns
/// therefore gets none of that behaviour, rather than inheriting block's shape
/// and writing SQL against columns its table does not have.
#[derive(Clone, Debug)]
pub struct WriteSchema {
    columns: HashSet<String>,
    /// The tombstone column of a type that declared soft-delete semantics
    /// ([`holon_api::entity::SoftDelete`]). `Some` turns `delete` into a
    /// tombstone write and adds `purge` as the op that removes the row.
    soft_delete: Option<String>,
    /// The entity references a `create` may carry.
    create_references: Vec<OperationParam>,
    /// The entity references an `update` may carry besides its `id`.
    update_references: Vec<OperationParam>,
    /// What `set_field`'s `value` is, by the field it writes.
    set_field_value: TypeHint,
}

/// A write's params, routed by [`WriteSchema::partition`].
pub struct PartitionedParams {
    pub columns: Vec<(String, Value)>,
    /// Params outside the column set plus the submitted bag's entries, the
    /// loose params winning.
    pub overflow: HashMap<String, Value>,
}

impl WriteSchema {
    /// The JSON column that absorbs params outside [`Self::is_column`]. An
    /// entity whose table lacks it has NO overflow, so an unknown param is an
    /// error.
    pub const OVERFLOW_COLUMN: &'static str = holon_api::FieldSchema::OVERFLOW_PROPERTIES;

    pub fn new(
        columns: impl IntoIterator<Item = String>,
        create_references: Vec<OperationParam>,
        update_references: Vec<OperationParam>,
        set_field_value: TypeHint,
    ) -> Self {
        Self {
            columns: columns.into_iter().collect(),
            soft_delete: None,
            create_references,
            update_references,
            set_field_value,
        }
    }

    pub fn from_references(
        columns: impl IntoIterator<Item = String>,
        type_def: &TypeDefinition,
    ) -> Result<Self> {
        Ok(Self::new(
            columns,
            holon_api::create_references(type_def)?,
            holon_api::row_field_references(type_def)?,
            holon_api::set_field_value_hint(type_def)?,
        ))
    }

    /// The vocabulary a runtime-declared type's raw table exposes: its
    /// PERSISTED fields. Computed and transient fields are not written.
    pub fn from_type_def(type_def: &TypeDefinition) -> Result<Self> {
        Ok(Self {
            soft_delete: type_def
                .soft_delete
                .as_ref()
                .map(|s| s.tombstone_field.to_string()),
            ..Self::from_references(
                type_def
                    .persistent_fields()
                    .into_iter()
                    .map(|f| f.name.clone()),
                type_def,
            )?
        })
    }

    pub fn is_column(&self, name: &str) -> bool {
        self.columns.contains(name)
    }

    pub fn has_overflow(&self) -> bool {
        self.columns.contains(Self::OVERFLOW_COLUMN)
    }

    /// The tombstone column `delete` stamps instead of removing the row, for a
    /// type that declared soft-delete semantics.
    pub fn tombstone_column(&self) -> Option<&str> {
        self.soft_delete.as_deref()
    }

    /// Whether a create stamps write-time timestamps: only when the table
    /// declares the columns to hold them.
    pub fn stamps_timestamps(&self) -> bool {
        self.is_column("created_at") && self.is_column("updated_at")
    }

    pub fn column_list(&self) -> Vec<&str> {
        let mut cols: Vec<&str> = self.columns.iter().map(String::as_str).collect();
        cols.sort_unstable();
        cols
    }

    /// Where a param or `set_field` field is written: a real column, or the
    /// `properties` overflow. A type declaring neither has nowhere to put it —
    /// an error at the write boundary, never a write to a column the store
    /// does not have (nor a value quietly dropped into a bag nobody reads).
    /// `store` names where the columns live, for the message.
    pub fn require_write_route(&self, entity: &str, store: &str, field: &str) -> Result<()> {
        if self.is_column(field) || self.has_overflow() {
            return Ok(());
        }
        Err(format!(
            "{entity}: field '{field}' is not a column of `{store}` and `{entity}` declares no \
             `{overflow}` overflow column, so this write has nowhere to land. Declared columns: \
             {cols:?}. Add the field to the type definition, or write it to a type that has it.",
            overflow = Self::OVERFLOW_COLUMN,
            cols = self.column_list(),
        )
        .into())
    }

    /// Route a write's params into columns and the overflow bag.
    ///
    /// Operation-control params are dropped: they are interpreted, never
    /// stored. `claim` sees every other param first; one it claims (an edge
    /// field the caller stores elsewhere) is routed by the caller, not here.
    pub fn partition(
        &self,
        entity: &str,
        store: &str,
        params: &StorageEntity,
        mut claim: impl FnMut(&str, &Value) -> Result<bool>,
    ) -> Result<PartitionedParams> {
        let mut columns = Vec::new();
        let mut overflow = HashMap::new();
        let mut submitted_bag: Option<String> = None;

        for (key, value) in params.iter() {
            if &**key == Self::OVERFLOW_COLUMN {
                // Live callers pass the bag as a JSON string, but `capture_row`
                // hands it back DECODED, as an Object — reading only the string
                // shape dropped every property from a delete-inverse, so undo
                // resurrected the row stripped of them.
                submitted_bag = match value {
                    Value::Object(_) => Some(value.to_json_string()?),
                    _ => value.as_string().map(str::to_string),
                };
            } else if is_operation_control_param(key) {
                // Shared with the intent boundaries that REFUSE these keys, so
                // the "interpreted, not stored" set has exactly one definition.
            } else if claim(key, value)? {
            } else if self.is_column(key) {
                columns.push((key.to_string(), value.clone()));
            } else {
                self.require_write_route(entity, store, key)?;
                overflow.insert(key.to_string(), value.clone());
            }
        }

        if let Some(Ok(map)) = submitted_bag
            .map(|json_str| serde_json::from_str::<HashMap<String, serde_json::Value>>(&json_str))
        {
            for (k, v) in map {
                overflow
                    .entry(k)
                    .or_insert_with(|| Value::from_json_value(v));
            }
        }

        // ONE choke-point for both authored routes into the bag (loose params
        // and the submitted blob): the reserved removal marker read back from
        // storage would be a removal INSTRUCTION in stored state.
        for (key, value) in &overflow {
            value.reject_reserved_marker(key)?;
            value.reject_non_finite_float(key)?;
        }

        Ok(PartitionedParams { columns, overflow })
    }
}

/// The params of `replace_scope`: every row whose `owner_column` holds
/// `owner_value` is replaced by `rows` in one write, so a reader sees the
/// scope's whole old row set or its whole new one.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeReplacement {
    pub owner_column: String,
    pub owner_value: String,
    pub rows: Vec<StorageEntity>,
}

impl ScopeReplacement {
    pub const OP: &'static str = "replace_scope";

    /// Refuses a row without an id or outside the scope.
    pub fn new(
        owner_column: String,
        owner_value: String,
        rows: Vec<StorageEntity>,
    ) -> Result<Self> {
        let scope = Self {
            owner_column,
            owner_value,
            rows,
        };
        for row in &scope.rows {
            Self::id_of(row)?;
            if !scope.holds(row) {
                return Err(format!(
                    "{}: the row {:?} does not hold {:?} = {:?}, so it is not in the scope it \
                     replaces",
                    Self::OP,
                    row.get("id"),
                    scope.owner_column,
                    scope.owner_value
                )
                .into());
            }
        }
        Ok(scope)
    }

    pub fn parse(params: &StorageEntity) -> Result<Self> {
        let text = |name: &str| -> Result<String> {
            params
                .get(name)
                .and_then(|v| v.as_string())
                .map(str::to_string)
                .ok_or_else(|| format!("{}: missing text parameter '{name}'", Self::OP).into())
        };
        let rows = params
            .get("rows")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("{}: missing list parameter 'rows'", Self::OP))?
            .iter()
            .map(|row| {
                row.as_object()
                    .map(|fields| {
                        fields
                            .iter()
                            .map(|(k, v)| (k.as_str().into(), v.clone()))
                            .collect()
                    })
                    .ok_or_else(|| format!("{}: a row is {row:?}, not an object", Self::OP))
            })
            .collect::<std::result::Result<Vec<StorageEntity>, String>>()?;
        Self::new(text("owner_column")?, text("owner_value")?, rows)
    }

    pub fn params(&self) -> StorageEntity {
        let rows = self
            .rows
            .iter()
            .map(|row| {
                Value::Object(
                    row.iter()
                        .map(|(k, v)| (k.to_string(), v.clone()))
                        .collect(),
                )
            })
            .collect();
        StorageEntity::from([
            (
                "owner_column".into(),
                Value::String(self.owner_column.clone()),
            ),
            (
                "owner_value".into(),
                Value::String(self.owner_value.clone()),
            ),
            ("rows".into(), Value::Array(rows)),
        ])
    }

    /// Whether `row` belongs to this scope.
    pub fn holds(&self, row: &StorageEntity) -> bool {
        row.get(self.owner_column.as_str())
            .and_then(|v| v.as_string())
            == Some(self.owner_value.as_str())
    }

    pub fn id_of(row: &StorageEntity) -> Result<&str> {
        row.get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| format!("{}: a row carries no id: {row:?}", Self::OP).into())
    }
}

/// The marking delta and arcs of `set_field`, `create` and `delete`, for a
/// catalog whose net declares them.
pub struct CrudDeclarations {
    pub set_field: (MarkingDelta, TransitionArcs),
    pub create: (MarkingDelta, TransitionArcs),
    pub delete: (MarkingDelta, TransitionArcs),
}

impl CrudDeclarations {
    pub fn undeclared() -> Self {
        let undeclared = || (MarkingDelta::Undeclared, TransitionArcs::Undeclared);
        Self {
            set_field: undeclared(),
            create: undeclared(),
            delete: undeclared(),
        }
    }
}

/// The CRUD descriptors an entity's write authority advertises: `set_field`,
/// `create`, `update`, `delete`, `purge` when the schema declares a tombstone,
/// and `replace_scope` for a global type.
pub fn crud_descriptors(
    entity_name: &EntityName,
    entity_short_name: &str,
    target_scope: TargetScope,
    schema: &WriteSchema,
    declarations: CrudDeclarations,
) -> Vec<OperationDescriptor> {
    let id_param = || OperationParam {
        name: "id".to_string(),
        type_hint: TypeHint::EntityId {
            entity_name: entity_name.clone(),
        },
        description: "Entity ID".to_string(),
    };
    let internal = || holon_api::MenuExposure::NotListed {
        surface: holon_api::NonMenuSurface::Internal,
    };
    let (set_field_delta, set_field_arcs) = declarations.set_field;
    let (create_delta, create_arcs) = declarations.create;
    let (delete_delta, delete_arcs) = declarations.delete;

    let mut ops = vec![
        OperationDescriptor {
            entity_name: entity_name.clone(),
            entity_short_name: entity_short_name.to_string(),
            name: "set_field".to_string(),
            display_name: "Set Field".to_string(),
            description: format!("Set a field on {entity_short_name}"),
            required_params: vec![
                id_param(),
                OperationParam {
                    name: "field".to_string(),
                    type_hint: TypeHint::String,
                    description: "Field name".to_string(),
                },
                OperationParam {
                    name: "value".to_string(),
                    type_hint: schema.set_field_value.clone(),
                    description: "Field value".to_string(),
                },
            ],
            optional_params: vec![],
            id_column: "id".to_string(),
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope,
            boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
            menu_exposure: internal(),
            trigger: None,
            bound_params: Default::default(),
            marking_delta: set_field_delta,
            guard: holon_api::pattern::OpGuard::None,
            arcs: set_field_arcs,
        },
        OperationDescriptor {
            entity_name: entity_name.clone(),
            entity_short_name: entity_short_name.to_string(),
            name: "create".to_string(),
            display_name: "Create".to_string(),
            description: format!("Create a new {entity_short_name}"),
            id_column: "id".to_string(),
            required_params: vec![],
            optional_params: schema.create_references.clone(),
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope,
            boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
            menu_exposure: internal(),
            trigger: None,
            bound_params: Default::default(),
            marking_delta: create_delta,
            guard: holon_api::pattern::OpGuard::None,
            arcs: create_arcs,
        },
        crate::block_op_catalog::update_descriptor(
            entity_name,
            entity_short_name,
            target_scope,
            schema.update_references.clone(),
        ),
        OperationDescriptor {
            entity_name: entity_name.clone(),
            entity_short_name: entity_short_name.to_string(),
            name: "delete".to_string(),
            display_name: "Delete".to_string(),
            description: format!("Delete {entity_short_name}"),
            required_params: vec![id_param()],
            optional_params: vec![],
            id_column: "id".to_string(),
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope,
            boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
            menu_exposure: holon_api::MenuExposure::Listed {
                surfaces: holon_api::SurfaceSet {
                    slash_menu: true,
                    action_bar: false,
                },
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: delete_delta,
            guard: holon_api::pattern::OpGuard::None,
            arcs: delete_arcs,
        },
    ];

    if schema.tombstone_column().is_some() {
        ops.push(OperationDescriptor {
            entity_name: entity_name.clone(),
            entity_short_name: entity_short_name.to_string(),
            name: "purge".to_string(),
            display_name: "Purge".to_string(),
            description: format!("Remove a tombstoned {entity_short_name} for good"),
            required_params: vec![id_param()],
            optional_params: vec![],
            id_column: "id".to_string(),
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope,
            boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
            menu_exposure: internal(),
            trigger: None,
            bound_params: Default::default(),
            marking_delta: MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: TransitionArcs::Undeclared,
        });
    }
    // A scope is a set of rows in one table, which only a global type has.
    if target_scope == TargetScope::Global {
        let text_param = |name: &str, description: &str| OperationParam {
            name: name.to_string(),
            type_hint: TypeHint::String,
            description: description.to_string(),
        };
        ops.push(OperationDescriptor {
            entity_name: entity_name.clone(),
            entity_short_name: entity_short_name.to_string(),
            name: ScopeReplacement::OP.to_string(),
            display_name: "Replace Scope".to_string(),
            description: format!(
                "Replace every {entity_short_name} whose owner column holds a value with new rows, \
                 in one write"
            ),
            required_params: vec![
                text_param("owner_column", "The column that scopes the rows"),
                text_param("owner_value", "The value the scope's rows hold in it"),
                OperationParam {
                    name: "rows".to_string(),
                    type_hint: TypeHint::Collection,
                    description: "The scope's new rows".to_string(),
                },
            ],
            optional_params: vec![],
            id_column: "id".to_string(),
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope,
            boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
            menu_exposure: internal(),
            trigger: None,
            bound_params: Default::default(),
            marking_delta: MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: TransitionArcs::Undeclared,
        });
    }
    ops
}

#[cfg(test)]
mod tests {
    use holon_api::FieldSchema;

    use super::*;

    fn declared(name: &str, columns: &[&str]) -> TypeDefinition {
        let mut fields = vec![FieldSchema::new("id", "TEXT").primary_key()];
        fields.extend(
            columns
                .iter()
                .map(|c| FieldSchema::new(*c, "TEXT").nullable()),
        );
        TypeDefinition::new(name, fields)
    }

    /// The derivation reads the type's PERSISTED fields — nothing else. A
    /// column named here can never come from block's vocabulary.
    #[test]
    fn a_declared_types_vocabulary_is_exactly_its_persisted_fields() {
        let schema =
            WriteSchema::from_type_def(&declared("gen_1", &["aaa", "bbb"])).expect("schema");
        assert_eq!(schema.column_list(), vec!["aaa", "bbb", "id"]);
    }

    #[test]
    fn a_scope_replacement_round_trips_and_refuses_a_row_of_another_scope() {
        let row = |id: &str, owner: &str| {
            StorageEntity::from([
                ("id".into(), Value::String(id.into())),
                ("recipe_id".into(), Value::String(owner.into())),
            ])
        };
        let scope = ScopeReplacement::new(
            "recipe_id".into(),
            "r1".into(),
            vec![row("a", "r1"), row("b", "r1")],
        )
        .expect("both rows are in the scope");
        assert_eq!(ScopeReplacement::parse(&scope.params()).unwrap(), scope);

        let refused = ScopeReplacement::new("recipe_id".into(), "r1".into(), vec![row("c", "r2")]);
        assert!(refused.is_err(), "a row of scope r2 replaced scope r1");
    }
}
