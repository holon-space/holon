use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::types::EntityName;
use crate::Value;

/// flutter_rust_bridge:ignore
pub type PreconditionChecker = dyn Fn(&HashMap<String, Box<dyn std::any::Any + Send + Sync>>) -> Result<bool, String>
    + Send
    + Sync;

/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderSpec {
    /// Named views for multi-view rendering. Empty for single-view queries.
    #[serde(default)]
    pub views: HashMap<String, ViewSpec>,
    /// Default view name (first view, or "default" for single-view)
    #[serde(default = "default_view_name")]
    pub default_view: String,
    /// Root render expression (backward compatibility for single-view queries)
    /// For multi-view queries, use `views[default_view].structure` instead.
    /// The compiler sets `root = views[default_view].structure` for multi-view queries.
    pub root: RenderExpr,
    pub nested_queries: Vec<String>,
    /// Per-row templates for heterogeneous UNION queries.
    /// Each template has an index that corresponds to the `ui` column value in SQL results.
    /// Operations are wired based on each template's source entity.
    #[serde(default)]
    pub row_templates: Vec<RowTemplate>,
}

fn default_view_name() -> String {
    "default".to_string()
}

impl RenderSpec {
    /// Backward compatibility: get the single/default view's structure
    ///
    /// flutter_rust_bridge:ignore
    pub fn root(&self) -> Option<&RenderExpr> {
        if self.views.is_empty() {
            Some(&self.root)
        } else {
            self.views.get(&self.default_view).map(|v| &v.structure)
        }
    }

    /// A default table render spec for raw SQL queries without an explicit render spec.
    ///
    /// flutter_rust_bridge:ignore
    pub fn table() -> Self {
        Self {
            views: HashMap::new(),
            default_view: "default".to_string(),
            root: RenderExpr::FunctionCall {
                name: "table".to_string(),
                args: Vec::new(),
            },
            nested_queries: Vec::new(),
            row_templates: Vec::new(),
        }
    }

    /// Get mutable reference to root render expression.
    /// For single-view: mutates root field.
    /// For multi-view: mutates default view's structure.
    ///
    /// flutter_rust_bridge:ignore
    pub fn get_root_mut(&mut self) -> &mut RenderExpr {
        if self.views.is_empty() {
            &mut self.root
        } else {
            &mut self
                .views
                .get_mut(&self.default_view)
                .expect("RenderSpec must have default_view")
                .structure
        }
    }
}

/// Specification for a named view in multi-view rendering.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewSpec {
    /// Filter expression to select rows for this view (evaluated client-side)
    pub filter: Option<FilterExpr>,
    /// The collection render expression (list, tree, table, etc.)
    pub structure: RenderExpr,
}

/// Filter expression for selecting rows in a view.
/// Evaluated client-side on each row to determine if it belongs to the view.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FilterExpr {
    /// Column equals literal: this.region == "sidebar"
    Eq { column: String, value: Value },
    /// Column not equals
    Ne { column: String, value: Value },
    /// Boolean AND of filters
    And(Vec<FilterExpr>),
    /// Boolean OR of filters
    Or(Vec<FilterExpr>),
    /// Always true (no filter)
    All,
}

/// Per-row UI template for heterogeneous data rendering.
///
/// When a PRQL query uses `derive { ui = (render ...) }` after a `from <table>`,
/// the compiler extracts the render expression and assigns it an index.
/// The SQL output will have `<index> as ui` for that table's rows.
/// At render time, Flutter looks up `row['ui']` to find the right template.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowTemplate {
    /// Index used in the `ui` column to identify this template
    pub index: usize,
    /// Source entity name (e.g., "todoist_task", "todoist_project")
    /// Used for wiring operations to the correct entity
    pub entity_name: EntityName,
    /// Short name for entity-typed params (e.g., "task", "project")
    /// Used for generating drop target params like "task_id", "project_id"
    pub entity_short_name: String,
    /// The render expression for this entity
    pub expr: RenderExpr,
}

/// Resolved per-row profile from EntityProfile system.
///
/// Unlike RowTemplate (which is compile-time from PRQL UNION queries),
/// RowProfile is resolved at runtime based on row data and Rhai conditions.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowProfile {
    /// Profile name (e.g., "default", "task", "source")
    pub name: String,
    /// The render expression for this profile
    pub render: RenderExpr,
    /// Operations available for rows matching this profile
    pub operations: Vec<OperationDescriptor>,
}

/// Complete metadata for an operation
///
/// Generated by #[operations_trait] macro.
/// flutter_rust_bridge:non_opaque
#[derive(Clone, Serialize, Deserialize)]
pub struct OperationDescriptor {
    // Entity and table identification
    pub entity_name: EntityName, // "todoist_task", "block"
    /// Short name for entity-typed params (e.g., "task" for task_id, "project" for project_id)
    pub entity_short_name: String,
    pub id_column: String, // "id"

    // Operation metadata
    pub name: String,         // "set_state", "indent", "create"
    pub display_name: String, // "Mark as complete", "Indent"
    pub description: String,  // Human-readable description for UI
    pub required_params: Vec<OperationParam>,
    /// Fields that this operation affects (for pie menu auto-attachment)
    pub affected_fields: Vec<String>, // ["is_collapsed"], ["parent_id", "depth", "sort_key"], etc.
    /// How to derive required params from alternative sources (e.g., tree_position → parent_id)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_mappings: Vec<ParamMapping>,

    /// flutter_rust_bridge:opaque
    #[serde(skip_serializing, skip_deserializing)]
    pub precondition: Option<Arc<Box<PreconditionChecker>>>,
}

// Manual PartialEq: `precondition` is a `dyn Fn` which can't implement PartialEq.
impl PartialEq for OperationDescriptor {
    fn eq(&self, other: &Self) -> bool {
        self.entity_name == other.entity_name
            && self.entity_short_name == other.entity_short_name
            && self.id_column == other.id_column
            && self.name == other.name
            && self.display_name == other.display_name
            && self.description == other.description
            && self.required_params == other.required_params
            && self.affected_fields == other.affected_fields
            && self.param_mappings == other.param_mappings
    }
}

impl OperationDescriptor {
    /// Convert to an OperationWiring with default widget type.
    pub fn to_default_wiring(self) -> OperationWiring {
        OperationWiring {
            widget_type: WidgetType::Button,
            modified_param: String::new(),
            descriptor: self,
        }
    }
}

impl std::fmt::Debug for OperationDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OperationDescriptor")
            .field("entity_name", &self.entity_name)
            .field("entity_short_name", &self.entity_short_name)
            .field("id_column", &self.id_column)
            .field("name", &self.name)
            .field("display_name", &self.display_name)
            .field("description", &self.description)
            .field("required_params", &self.required_params)
            .field("affected_fields", &self.affected_fields)
            .field("param_mappings", &self.param_mappings)
            .field(
                "precondition",
                &self.precondition.as_ref().map(|_| "<closure>"),
            )
            .finish()
    }
}

/// An executable operation with all parameters
///
/// Operations can be executed through the OperationProvider trait,
/// and each operation can return its inverse operation for undo support.
/// flutter_rust_bridge:non_opaque
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Operation {
    /// Entity name (e.g., "todoist_task", "block")
    pub entity_name: EntityName,
    /// Operation name (e.g., "move_block", "set_state")
    pub op_name: String,
    /// Human-readable display name for UI (e.g., "Move block", "Complete task")
    pub display_name: String,
    /// Operation parameters as key-value pairs
    pub params: HashMap<String, Value>,
}

impl Operation {
    /// Create a new operation
    pub fn new(
        entity_name: impl Into<EntityName>,
        op_name: impl Into<String>,
        display_name: impl Into<String>,
        params: HashMap<String, Value>,
    ) -> Self {
        Self {
            entity_name: entity_name.into(),
            op_name: op_name.into(),
            display_name: display_name.into(),
            params,
        }
    }

    /// Create an operation from a hashmap (convenience method)
    pub fn from_params(
        entity_name: impl Into<EntityName>,
        op_name: impl Into<String>,
        display_name: impl Into<String>,
        params: impl IntoIterator<Item = (String, Value)>,
    ) -> Self {
        Self {
            entity_name: entity_name.into(),
            op_name: op_name.into(),
            display_name: display_name.into(),
            params: params.into_iter().collect(),
        }
    }

    /// Set the entity name (useful when entity_name is not known at construction time)
    pub fn with_entity_name(mut self, entity_name: impl Into<EntityName>) -> Self {
        self.entity_name = entity_name.into();
        self
    }
}

/// Type hints for operation parameters
///
/// Encodes whether a parameter is a primitive value or an entity reference.
/// Entity references enable the test infrastructure to track dependencies.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TypeHint {
    /// Boolean value
    Bool,
    /// String value
    String,
    /// Numeric value (integer)
    Number,
    /// Reference to an entity ID
    ///
    /// Example: `EntityId { entity_name: "project" }` means this parameter
    /// must be the ID of a "project" entity.
    EntityId { entity_name: EntityName },
    /// One-of constraint: parameter must be one of the provided values
    ///
    /// Example: `OneOf { values: [...] }` means this parameter must be one of the listed values.
    /// Values can be strings, objects (like CompletionStateInfo), or any other Value type.
    /// Used for state fields, priority levels, etc.
    OneOf { values: Vec<Value> },
    /// Nested object with sub-fields.
    ///
    /// Produced from JSON Schema `"type": "object"` with `"properties"`.
    /// Flutter UI rendering is a follow-up task; for now this enables schema introspection.
    Object { fields: Vec<OperationParam> },
    /// Unevaluated expression (lazy computation / template).
    /// Used by widget builders for args that should remain as RenderExpr
    /// rather than being evaluated to a Value (e.g., item_template, sort_key).
    Expr,
    /// Items from per-row expansion.
    /// Indicates this parameter represents a collection of items that
    /// should be lazily expanded from data rows using a template.
    Collection,
}

impl TypeHint {
    /// Convert from legacy string format for backward compatibility
    pub fn from_string(s: &str) -> Self {
        match s {
            "bool" | "boolean" => TypeHint::Bool,
            "string" | "str" => TypeHint::String,
            "number" | "integer" | "int" | "i64" | "i32" => TypeHint::Number,
            s if s.starts_with("entity_id:") => {
                let entity_name = EntityName::new(s.strip_prefix("entity_id:").unwrap());
                TypeHint::EntityId { entity_name }
            }
            s if s.starts_with("enum:") => {
                let values_str = s.strip_prefix("enum:").unwrap();
                let string_values: Vec<String> = values_str
                    .split(',')
                    .map(|v| v.trim().to_string())
                    .collect();
                // Convert string values to Value::String for backward compatibility
                let values: Vec<Value> = string_values.into_iter().map(Value::String).collect();
                TypeHint::OneOf { values }
            }
            "expr" | "expression" | "template" => TypeHint::Expr,
            "collection" | "items" => TypeHint::Collection,
            _ => TypeHint::String, // Default fallback
        }
    }
}

/// Parameter descriptor for operation metadata
///
/// Describes a required parameter for an operation.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationParam {
    pub name: String, // "completed", "new_parent_id"
    #[serde(deserialize_with = "deserialize_type_hint")]
    pub type_hint: TypeHint, // Now enum instead of String
    pub description: String, // "Whether task is completed"
}

/// Describes how to derive required parameters from alternative sources.
///
/// Enables auto-discovery: widgets provide generic params (like `tree_position` or `selected_id`),
/// and operations declare how to map those to their specific `required_params`.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ParamMapping {
    /// Source param name from widget (e.g., "tree_position", "selected_id")
    pub from: String,
    /// Which required params this source provides (e.g., ["parent_id", "predecessor"])
    pub provides: Vec<String>,
    /// Default values for params not extractable from source
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub defaults: HashMap<String, Value>,
}

/// Custom deserializer for TypeHint that supports both old string format and new enum format
fn deserialize_type_hint<'de, D>(deserializer: D) -> Result<TypeHint, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct TypeHintVisitor;

    impl<'de> Visitor<'de> for TypeHintVisitor {
        type Value = TypeHint;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string or TypeHint enum")
        }

        fn visit_str<E>(self, value: &str) -> Result<TypeHint, E>
        where
            E: de::Error,
        {
            Ok(TypeHint::from_string(value))
        }

        fn visit_map<M>(self, mut map: M) -> Result<TypeHint, M::Error>
        where
            M: de::MapAccess<'de>,
        {
            // Delegate to default deserialization for enum format
            let mut type_field: Option<String> = None;
            let mut entity_name: Option<EntityName> = None;
            let mut values: Option<Vec<Value>> = None;
            let mut fields: Option<Vec<OperationParam>> = None;

            while let Some(key) = map.next_key::<String>()? {
                match key.as_str() {
                    "type" => {
                        type_field = Some(map.next_value()?);
                    }
                    "entity_name" => {
                        entity_name = Some(map.next_value()?);
                    }
                    "values" => {
                        values = Some(map.next_value()?);
                    }
                    "fields" => {
                        fields = Some(map.next_value()?);
                    }
                    _ => {
                        let _ = map.next_value::<de::IgnoredAny>()?;
                    }
                }
            }

            match type_field.as_deref() {
                Some("entity_id") | Some("EntityId") => {
                    let entity_name =
                        entity_name.ok_or_else(|| de::Error::missing_field("entity_name"))?;
                    Ok(TypeHint::EntityId { entity_name })
                }
                Some("one_of") | Some("OneOf") => {
                    let values = values.ok_or_else(|| de::Error::missing_field("values"))?;
                    Ok(TypeHint::OneOf { values })
                }
                Some("object") | Some("Object") => {
                    let fields = fields.ok_or_else(|| de::Error::missing_field("fields"))?;
                    Ok(TypeHint::Object { fields })
                }
                Some("bool") | Some("Bool") => Ok(TypeHint::Bool),
                Some("string") | Some("String") => Ok(TypeHint::String),
                Some("number") | Some("Number") => Ok(TypeHint::Number),
                Some("expr") | Some("Expr") => Ok(TypeHint::Expr),
                Some("collection") | Some("Collection") => Ok(TypeHint::Collection),
                // Backward compatibility: handle "enum" as "one_of"
                Some("enum") | Some("Enum") => {
                    let values = values.ok_or_else(|| de::Error::missing_field("values"))?;
                    Ok(TypeHint::OneOf { values })
                }
                _ => Err(de::Error::custom("Unknown type hint variant")),
            }
        }
    }

    deserializer.deserialize_any(TypeHintVisitor)
}

/// Widget type for operation wiring — determines which UI control renders the operation.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WidgetType {
    Checkbox,
    Text,
    Button,
}

impl std::fmt::Display for WidgetType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WidgetType::Checkbox => write!(f, "checkbox"),
            WidgetType::Text => write!(f, "text"),
            WidgetType::Button => write!(f, "button"),
        }
    }
}

impl std::str::FromStr for WidgetType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "checkbox" => Ok(WidgetType::Checkbox),
            "text" => Ok(WidgetType::Text),
            "button" => Ok(WidgetType::Button),
            other => anyhow::bail!(
                "Invalid widget type: {other:?} (expected \"checkbox\", \"text\", or \"button\")"
            ),
        }
    }
}

/// Connects lineage analysis results to operation metadata
///
/// Embedded in FunctionCall nodes in RenderSpec and sent to Flutter frontend.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationWiring {
    pub widget_type: WidgetType,
    pub modified_param: String,

    // Complete operation metadata (no duplication!)
    pub descriptor: OperationDescriptor,
}

/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
//#[serde(tag = "type", rename_all = "snake_case")]
pub enum RenderExpr {
    FunctionCall {
        name: String,
        args: Vec<Arg>,
    },
    /// Reference to another block — frontend calls render_block(block_id) to get its WidgetSpec
    BlockRef {
        block_id: String,
    },
    ColumnRef {
        name: String,
    },
    Literal {
        value: Value,
    },
    BinaryOp {
        op: BinaryOperator,
        left: Box<RenderExpr>,
        right: Box<RenderExpr>,
    },
    Array {
        items: Vec<RenderExpr>,
    },
    Object {
        fields: HashMap<String, RenderExpr>,
    },
}

impl RenderExpr {
    /// Serialize back to Rhai DSL syntax.
    ///
    /// Enables round-trip: `RenderExpr → to_rhai() → parse → RenderExpr`.
    /// Used by PBT to generate render source block content from typed expressions.
    ///
    /// flutter_rust_bridge:ignore
    pub fn to_rhai(&self) -> String {
        match self {
            RenderExpr::FunctionCall { name, args, .. } => {
                if args.is_empty() {
                    format!("{name}()")
                } else {
                    let positional: Vec<&Arg> = args.iter().filter(|a| a.name.is_none()).collect();
                    let named: Vec<&Arg> = args.iter().filter(|a| a.name.is_some()).collect();

                    let mut parts = Vec::new();
                    for a in &positional {
                        parts.push(a.value.to_rhai());
                    }
                    if !named.is_empty() {
                        let named_str = named
                            .iter()
                            .map(|a| format!("{}: {}", a.name.as_ref().unwrap(), a.value.to_rhai()))
                            .collect::<Vec<_>>()
                            .join(", ");
                        parts.push(format!("#{{{named_str}}}"));
                    }
                    format!("{name}({})", parts.join(", "))
                }
            }
            RenderExpr::BlockRef { block_id } => format!("block_ref(\"{block_id}\")"),
            RenderExpr::ColumnRef { name } => format!("col(\"{name}\")"),
            RenderExpr::Literal { value } => value_to_rhai(value),
            RenderExpr::BinaryOp { op, left, right } => {
                format!("{} {} {}", left.to_rhai(), op.to_rhai(), right.to_rhai())
            }
            RenderExpr::Array { items } => {
                let inner = items
                    .iter()
                    .map(|i| i.to_rhai())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{inner}]")
            }
            RenderExpr::Object { fields } => {
                let inner = fields
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v.to_rhai()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("#{{{inner}}}")
            }
        }
    }

    /// Recursively collect all `ColumnRef` names referenced by this expression.
    ///
    /// Used to determine which data columns a render template makes visible,
    /// so assertions can filter expected data to only comparable columns.
    ///
    /// flutter_rust_bridge:ignore
    pub fn visible_columns(&self) -> Vec<String> {
        let mut cols = Vec::new();
        self.collect_columns(&mut cols);
        cols
    }

    fn collect_columns(&self, out: &mut Vec<String>) {
        match self {
            RenderExpr::ColumnRef { name } => out.push(name.clone()),
            RenderExpr::FunctionCall { args, .. } => {
                for arg in args {
                    arg.value.collect_columns(out);
                }
            }
            RenderExpr::BinaryOp { left, right, .. } => {
                left.collect_columns(out);
                right.collect_columns(out);
            }
            RenderExpr::Array { items } => {
                for item in items {
                    item.collect_columns(out);
                }
            }
            RenderExpr::Object { fields } => {
                for expr in fields.values() {
                    expr.collect_columns(out);
                }
            }
            RenderExpr::BlockRef { .. } | RenderExpr::Literal { .. } => {}
        }
    }
}

fn value_to_rhai(value: &Value) -> String {
    match value {
        Value::String(s) => format!("\"{s}\""),
        Value::Integer(n) => n.to_string(),
        Value::Float(f) => format!("{f}"),
        Value::Boolean(b) => b.to_string(),
        Value::Null => "()".to_string(),
        Value::Array(items) => {
            let inner = items
                .iter()
                .map(value_to_rhai)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
        Value::Object(map) => {
            let inner = map
                .iter()
                .map(|(k, v)| format!("{k}: {}", value_to_rhai(v)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("#{{{inner}}}")
        }
        Value::DateTime(s) => format!("\"{s}\""),
        Value::Json(s) => format!("\"{s}\""),
    }
}

impl BinaryOperator {
    fn to_rhai(&self) -> &'static str {
        match self {
            BinaryOperator::Eq => "==",
            BinaryOperator::Neq => "!=",
            BinaryOperator::Gt => ">",
            BinaryOperator::Lt => "<",
            BinaryOperator::Gte => ">=",
            BinaryOperator::Lte => "<=",
            BinaryOperator::Add => "+",
            BinaryOperator::Sub => "-",
            BinaryOperator::Mul => "*",
            BinaryOperator::Div => "/",
            BinaryOperator::And => "&&",
            BinaryOperator::Or => "||",
        }
    }
}

/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Arg {
    pub name: Option<String>,
    pub value: RenderExpr,
}

/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOperator {
    Eq,
    Neq,
    Gt,
    Lt,
    Gte,
    Lte,
    Add,
    Sub,
    Mul,
    Div,
    And,
    Or,
}

/// A unified object combining row data and template.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderableItem {
    pub row_data: HashMap<String, Value>,
    pub template: RowTemplate,
}

impl RenderableItem {
    /// flutter_rust_bridge:ignore
    pub fn new(row_data: HashMap<String, Value>, template: RowTemplate) -> Self {
        Self { row_data, template }
    }
}

/// Recursively collect all widget (FunctionCall) names from a RenderExpr.
///
/// Used by ProfileResolver to check whether a frontend supports all widgets
/// referenced by a profile variant's render expression.
pub fn extract_widget_names(expr: &RenderExpr) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_widget_names(expr, &mut names);
    names
}

fn collect_widget_names(expr: &RenderExpr, out: &mut HashSet<String>) {
    match expr {
        RenderExpr::FunctionCall { name, args, .. } => {
            out.insert(name.clone());
            for arg in args {
                collect_widget_names(&arg.value, out);
            }
        }
        RenderExpr::BinaryOp { left, right, .. } => {
            collect_widget_names(left, out);
            collect_widget_names(right, out);
        }
        RenderExpr::Array { items } => {
            for item in items {
                collect_widget_names(item, out);
            }
        }
        RenderExpr::Object { fields } => {
            for expr in fields.values() {
                collect_widget_names(expr, out);
            }
        }
        RenderExpr::BlockRef { .. } | RenderExpr::ColumnRef { .. } | RenderExpr::Literal { .. } => {
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fc(name: &str, args: Vec<Arg>) -> RenderExpr {
        RenderExpr::FunctionCall {
            name: name.into(),
            args,
        }
    }

    fn named_arg(name: &str, value: RenderExpr) -> Arg {
        Arg {
            name: Some(name.into()),
            value,
        }
    }

    #[test]
    fn to_rhai_table() {
        assert_eq!(fc("table", vec![]).to_rhai(), "table()");
    }

    #[test]
    fn to_rhai_list_with_block_ref() {
        let expr = fc(
            "list",
            vec![named_arg("item_template", fc("block_ref", vec![]))],
        );
        assert_eq!(expr.to_rhai(), r#"list(#{item_template: block_ref()})"#);
    }

    #[test]
    fn to_rhai_columns_with_gap() {
        let expr = fc(
            "columns",
            vec![
                named_arg(
                    "gap",
                    RenderExpr::Literal {
                        value: Value::Integer(4),
                    },
                ),
                named_arg("item_template", fc("block_ref", vec![])),
            ],
        );
        assert_eq!(
            expr.to_rhai(),
            r#"columns(#{gap: 4, item_template: block_ref()})"#,
        );
    }

    #[test]
    fn to_rhai_nested_row_text_col() {
        let expr = fc(
            "list",
            vec![named_arg(
                "item_template",
                fc(
                    "row",
                    vec![Arg {
                        name: None,
                        value: fc(
                            "text",
                            vec![Arg {
                                name: None,
                                value: RenderExpr::ColumnRef {
                                    name: "content".into(),
                                },
                            }],
                        ),
                    }],
                ),
            )],
        );
        assert_eq!(
            expr.to_rhai(),
            r#"list(#{item_template: row(text(col("content")))})"#,
        );
    }

    #[test]
    fn to_rhai_column_ref() {
        assert_eq!(
            RenderExpr::ColumnRef { name: "id".into() }.to_rhai(),
            r#"col("id")"#,
        );
    }

    #[test]
    fn visible_columns_extracts_column_refs() {
        let expr = fc(
            "row",
            vec![
                Arg {
                    name: None,
                    value: fc(
                        "text",
                        vec![Arg {
                            name: None,
                            value: RenderExpr::ColumnRef {
                                name: "content".into(),
                            },
                        }],
                    ),
                },
                Arg {
                    name: None,
                    value: fc(
                        "badge",
                        vec![Arg {
                            name: None,
                            value: RenderExpr::ColumnRef {
                                name: "task_state".into(),
                            },
                        }],
                    ),
                },
            ],
        );
        let cols = expr.visible_columns();
        assert_eq!(cols, vec!["content", "task_state"]);
    }

    #[test]
    fn visible_columns_empty_for_block_ref() {
        let expr = fc(
            "list",
            vec![named_arg("item_template", fc("block_ref", vec![]))],
        );
        assert!(expr.visible_columns().is_empty());
    }
}
