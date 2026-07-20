use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;

use crate::Value;
use crate::predicate::Predicate;
use crate::types::EntityName;

/// flutter_rust_bridge:ignore
pub type PreconditionChecker = dyn Fn(&HashMap<String, Box<dyn std::any::Any + Send + Sync>>) -> Result<bool, String>
    + Send
    + Sync;

/// Specification for a named view in multi-view rendering.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewSpec {
    /// Filter predicate to select rows for this view (evaluated client-side)
    pub filter: Option<Predicate>,
    /// The collection render expression (list, tree, table, etc.)
    pub structure: RenderExpr,
}

/// A render variant candidate — one way to render an entity, with a condition
/// that the frontend evaluates to pick the active variant.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderVariant {
    pub name: String,
    pub render: RenderExpr,
    pub operations: Vec<OperationDescriptor>,
    /// Frontend-evaluated condition (UI state: focus, view mode, etc.)
    pub condition: Predicate,
}

/// One per-row override rule on a builder (today: `tree`).
///
/// Schema in DSL:
/// ```text
/// rules: [#{
///   when: #{eq: #{field: "level", value: 0}},
///   override: #{role: "page_title", show_bullet: false}
/// }]
/// ```
///
/// All rules whose `when` matches a row contribute their `overrides` map.
/// Later rules' keys override earlier rules' keys per key (all-matches-merge).
/// Builders consume the merged map for both render-context flags (e.g. `role`
/// is read by `pick_active_variant`) and chrome props (`show_bullet`,
/// `show_chevron` on tree_item).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSpec {
    pub when: Predicate,
    /// `override` is a Rust keyword; field renamed to `overrides`. Serde keeps
    /// the user-facing DSL field name `override` via `rename`.
    #[serde(rename = "override")]
    pub overrides: std::collections::HashMap<String, Value>,
}

/// Per-row UI template for heterogeneous data rendering.
///
/// When a PRQL query uses `derive { ui = (render ...) }` after a `from
/// <table>`, the compiler extracts the render expression and assigns it an
/// index. The SQL output will have `<index> as ui` for that table's rows.
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
/// RenderProfile is resolved at runtime based on row data and Rhai conditions.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderProfile {
    /// Profile name (first variant name for backward compat)
    pub name: String,
    /// The render expression (first variant render for backward compat)
    pub render: RenderExpr,
    /// Operations available for rows matching this profile
    pub operations: Vec<OperationDescriptor>,
    /// All matching variants (frontend picks based on local UI state)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<RenderVariant>,
}

// ALLOW(compatibility): RowProfile is a deprecated alias still used by
// serialized fixtures
/// Deprecated alias retained for serialized fixtures.
pub type RowProfile = RenderProfile;

/// Modifier keys held during a mouse click.
///
/// Carried inside `Trigger::Click` so the same widget can route different
/// modifier combinations to different operations (e.g. plain click =
/// `navigation.focus`, shift+click = `navigation.focus_pin`). Adding a new
/// modifier just sets another field — no new `Trigger` variant required.
///
/// `ClickModifiers::none()` denotes a primary click; the named constructors
/// (`shift()`, `alt()`, `cmd()`, `ctrl()`) cover the single-modifier cases
/// that have actually shipped. For combined modifiers, construct directly
/// (e.g. `ClickModifiers { shift: true, alt: true, ..Default::default() }`).
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClickModifiers {
    pub shift: bool,
    pub alt: bool,
    pub cmd: bool,
    pub ctrl: bool,
}

impl ClickModifiers {
    pub const fn none() -> Self {
        Self {
            shift: false,
            alt: false,
            cmd: false,
            ctrl: false,
        }
    }

    pub const fn shift() -> Self {
        Self {
            shift: true,
            alt: false,
            cmd: false,
            ctrl: false,
        }
    }

    pub const fn alt() -> Self {
        Self {
            shift: false,
            alt: true,
            cmd: false,
            ctrl: false,
        }
    }

    pub const fn cmd() -> Self {
        Self {
            shift: false,
            alt: false,
            cmd: true,
            ctrl: false,
        }
    }

    pub const fn ctrl() -> Self {
        Self {
            shift: false,
            alt: false,
            cmd: false,
            ctrl: true,
        }
    }

    pub const fn is_none(&self) -> bool {
        !self.shift && !self.alt && !self.cmd && !self.ctrl
    }
}

/// Input that invokes an operation when bound to a widget.
///
/// `OperationWiring` carries one of these on its descriptor; the input
/// pipeline matches incoming events against the trigger to pick which op to
/// dispatch. `None` means the op is dispatched programmatically only (no
/// widget binding).
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Trigger {
    /// A keyboard chord (e.g. Cmd+Enter).
    KeyChord { chord: crate::input_types::KeyChord },
    /// A mouse click with the given modifier keys held. `modifiers ==
    /// ClickModifiers::none()` is a primary click; non-empty modifiers (e.g.
    /// `ClickModifiers::shift()` for "open in side panel" / "pin" gestures)
    /// route to a different op. Frontends `stop_propagation` on any
    /// modifier-click path so the row-level click handler doesn't also fire.
    Click { modifiers: ClickModifiers },
}

/// Where an operation surfaces in the UI — declared AT the descriptor so a new
/// op cannot ship without deciding its discoverability. This is the
/// parse-don't-validate replacement for the implicit "any op in the profile is
/// a menu candidate" rule: the correspondence test asserts the rendered slash
/// menu == exactly the `Listed` ops resolvable in context, so a regression like
/// "the menu silently collapsed to one entry" (GPUI dogfood 2026-07-20, bug b)
/// fails a compile-checked oracle instead of shipping.
///
/// `OperationDescriptor` deliberately has NO `Default`, so every construction
/// site MUST classify explicitly — a forgotten classification is a compile
/// error, not a silent default.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MenuExposure {
    /// Appears in the slash command menu whenever its params resolve from the
    /// editor context (indent, outdent, move_up, move_down, delete, convert…).
    ///
    /// Carries a `SurfaceSet` so ONE exposure axis drives both the slash menu
    /// and the (future) mobile action bar — single source of truth, no parallel
    /// enum. Today every `Listed` op is `slash_menu: true, action_bar: false`,
    /// preserving exact current behaviour (slash-only, invisible to the
    /// not-yet-existent action bar).
    Listed { surfaces: SurfaceSet },
    /// Not a bare menu op — surfaced only through a dedicated picker whose
    /// entries are data-driven (e.g. per-template rows for
    /// `instantiate_template`).
    PickerBacked { picker: PickerKind },
    /// Deliberately absent from the slash menu: reachable via another surface
    /// only (keyboard/pointer gesture, navigation, sync, or an internal
    /// read-only planner step).
    NotListed { surface: NonMenuSurface },
}

/// Which UI surfaces a `Listed` op is reachable from. One exposure axis drives
/// both the slash menu and the (future) mobile action bar, so an op's
/// discoverability lives in a single place instead of two parallel enums.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceSet {
    /// Surfaces in the slash command menu.
    pub slash_menu: bool,
    /// Surfaces in the mobile action bar (not yet rendered).
    pub action_bar: bool,
}

/// How narrowly an operation targets — the sort key for the future action bar.
///
/// Derive order is narrowness order: `Block < Page < Global`. `Block`-acting
/// ops (cycle-state, indent/outdent, move, delete, embed, convert-to-page) act
/// on a single block; `Page`-level ops (share, rename-page, page settings) act
/// on a page; app/global ops act on the whole app.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetScope {
    Block,
    Page,
    Global,
}

/// A dedicated picker that surfaces an op outside the flat command list.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickerKind {
    /// The template-instantiation picker (per-template `Template: <name>`
    /// rows).
    Template,
}

/// The non-menu surface an op is reachable from when it is `NotListed`.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonMenuSurface {
    /// Keyboard chord / typed-text editing gesture (split_block, type_chars…).
    KeyboardGesture,
    /// Pointer gesture (drag/drop, click-to-focus, expand/collapse toggle).
    PointerGesture,
    /// Navigation / view switching.
    Navigation,
    /// External sync, data ingest, or system/harness op.
    External,
    /// A read-only planner or internal compound step — not a user-facing op
    /// (e.g. `block_to_page_plan`, the read half of `convert_block_to_page`).
    Internal,
    /// A provider CRUD op not surfaced in the block slash menu — the default
    /// for macro-generated provider ops (todoist, etc.) that do not opt in via
    /// `#[menu_exposure(...)]`. Fail-closed: invisible until deliberately
    /// Listed.
    ProviderDefault,
    /// A test-only synthetic descriptor.
    Test,
}

/// Complete metadata for an operation
///
/// Generated by #[operations_trait] macro.
/// flutter_rust_bridge:non_opaque
#[derive(Clone, Serialize, Deserialize)]
/// @c4 code
pub struct OperationDescriptor {
    // Entity and table identification
    pub entity_name: EntityName, // "todoist_task", "block"
    /// Short name for entity-typed params (e.g., "task" for task_id, "project"
    /// for project_id)
    pub entity_short_name: String,
    pub id_column: String, // "id"

    // Operation metadata
    pub name: String,         // "set_state", "indent", "create"
    pub display_name: String, // "Mark as complete", "Indent"
    pub description: String,  // Human-readable description for UI
    pub required_params: Vec<OperationParam>,
    /// Fields that this operation affects (for pie menu auto-attachment)
    pub affected_fields: Vec<String>, // ["is_collapsed"], ["parent_id", "depth", "sort_key"], etc.
    /// How to derive required params from alternative sources (e.g.,
    /// tree_position → parent_id)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_mappings: Vec<ParamMapping>,

    /// Where this op surfaces in the UI. Non-defaultable (no `Default` on the
    /// struct) so every construction site classifies explicitly — the
    /// registry↔menu correspondence oracle reads this instead of assuming
    /// "every profile op is a menu candidate".
    pub menu_exposure: MenuExposure,

    /// How narrowly this op targets (block / page / global). Non-defaultable
    /// (no `Default` on the struct) so every construction site classifies
    /// explicitly — a forgotten scope is a compile error, not a silent default.
    /// The future action bar sorts its ops by this narrowness key.
    pub target_scope: TargetScope,

    /// Input that invokes this operation when bound to a widget.
    ///
    /// `KeyChord` is populated at ViewModel construction time from the
    /// reactive keybinding registry. `Click` is set explicitly by widgets
    /// that bind a click action via the render DSL (e.g. `selectable`'s
    /// `action:` arg). `None` means the op is dispatched programmatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Trigger>,

    /// Pre-resolved param values bound from DSL args at interpret time.
    ///
    /// Example: a selectable's
    /// `action: navigation_focus(#{region: "main", block_id: col("id")})`
    /// resolves the arg expressions against the row's data and stores the
    /// concrete `Value`s here. Merged into the runtime intent at dispatch
    /// time alongside the entity id and any auto-derived params.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub bound_params: HashMap<String, Value>,

    /// flutter_rust_bridge:opaque
    #[serde(skip_serializing, skip_deserializing)]
    pub precondition: Option<Arc<Box<PreconditionChecker>>>,
}

// Manual PartialEq: `precondition` is a `dyn Fn` which can't implement
// PartialEq.
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
            && self.menu_exposure == other.menu_exposure
            && self.trigger == other.trigger
            && self.bound_params == other.bound_params
    }
}

// NOTE: `OperationDescriptor` intentionally has NO `Default`. `menu_exposure`
// is non-defaultable — every construction site must classify its UI surface, so
// a forgotten classification is a compile error rather than an op that silently
// vanishes from (or leaks into) the slash menu.

impl OperationDescriptor {
    /// Convert to an OperationWiring with default widget type.
    pub fn to_default_wiring(self) -> OperationWiring {
        OperationWiring {
            modified_param: String::new(),
            descriptor: self,
        }
    }

    /// Returns the bound key chord, if this op is invoked by one.
    pub fn key_chord(&self) -> Option<&crate::input_types::KeyChord> {
        match &self.trigger {
            Some(Trigger::KeyChord { chord }) => Some(chord),
            _ => None,
        }
    }

    /// Returns the click modifiers if this op is click-triggered, else `None`.
    /// Mirrors `key_chord()` for the keyboard case. Use this when matching a
    /// runtime modifier set against bound operations.
    pub fn click_modifiers(&self) -> Option<ClickModifiers> {
        match &self.trigger {
            Some(Trigger::Click { modifiers }) => Some(*modifiers),
            _ => None,
        }
    }

    /// Returns true if this op is invoked by a primary click (no modifiers).
    pub fn is_click_triggered(&self) -> bool {
        self.click_modifiers().is_some_and(|m| m.is_none())
    }

    /// Returns true if this op is invoked by shift+click on the bound widget.
    pub fn is_shift_click_triggered(&self) -> bool {
        self.click_modifiers() == Some(ClickModifiers::shift())
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
            .field("menu_exposure", &self.menu_exposure)
            .field("trigger", &self.trigger)
            .field("bound_params", &self.bound_params)
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
/// @c4 code
/// @c4 uses OperationDescriptor "describes the op" "call"
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

    /// Set the entity name (useful when entity_name is not known at
    /// construction time)
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
    /// Example: `OneOf { values: [...] }` means this parameter must be one of
    /// the listed values. Values can be strings, objects (like
    /// CompletionStateInfo), or any other Value type. Used for state
    /// fields, priority levels, etc.
    OneOf { values: Vec<Value> },
    /// Nested object with sub-fields.
    ///
    /// Produced from JSON Schema `"type": "object"` with `"properties"`.
    /// Flutter UI rendering is a follow-up task; for now this enables schema
    /// introspection.
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
    // ALLOW(compatibility): legacy string format is still emitted by older fixtures
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
                // ALLOW(compatibility): legacy enum:foo,bar form predates the structured
                // Value::String list; convert string values to Value::String for
                // the legacy callers
                let values: Vec<Value> = string_values.into_iter().map(Value::String).collect();
                TypeHint::OneOf { values }
            }
            "expr" | "expression" | "template" => TypeHint::Expr,
            "collection" | "items" => TypeHint::Collection,
            _ => TypeHint::String, /* Default fallback // ALLOW(fallback): legacy string-format
                                    * conversion default */
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
/// Enables auto-discovery: widgets provide generic params (like `tree_position`
/// or `selected_id`), and operations declare how to map those to their specific
/// `required_params`. flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ParamMapping {
    /// Source param name from widget (e.g., "tree_position", "selected_id")
    pub from: String,
    /// Which required params this source provides (e.g., ["parent_id",
    /// "predecessor"])
    pub provides: Vec<String>,
    /// Default values for params not extractable from source
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub defaults: HashMap<String, Value>,
}

/// Custom deserializer for TypeHint that supports both old string format and
/// new enum format
fn deserialize_type_hint<'de, D>(deserializer: D) -> Result<TypeHint, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use std::fmt;

    use serde::de::Visitor;
    use serde::de::{self};

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
                // ALLOW(compatibility): older fixtures still serialize "enum" rather than "one_of"
                // Older fixtures: handle "enum" as "one_of"
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

/// Connects lineage analysis results to operation metadata
///
/// Embedded in FunctionCall nodes in RenderSpec and sent to Flutter frontend.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationWiring {
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
    /// Reference to another block — frontend calls render_entity(block_id) to
    /// get its WidgetSpec
    LiveBlock {
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
    /// Used by PBT to generate render source block content from typed
    /// expressions.
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
            RenderExpr::LiveBlock { block_id } => format!("live_block(\"{block_id}\")"),
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
            RenderExpr::LiveBlock { .. } | RenderExpr::Literal { .. } => {}
        }
    }

    /// Recursively collect every `LiveBlock` target block_id referenced by this
    /// expression. Used by the PBT to determine which panel blocks (and thus
    /// regions) the active root layout actually renders.
    ///
    /// flutter_rust_bridge:ignore
    pub fn live_block_targets(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_live_block_targets(&mut out);
        out
    }

    fn collect_live_block_targets(&self, out: &mut Vec<String>) {
        match self {
            RenderExpr::LiveBlock { block_id } => out.push(block_id.clone()),
            RenderExpr::FunctionCall { args, .. } => {
                for arg in args {
                    arg.value.collect_live_block_targets(out);
                }
            }
            RenderExpr::BinaryOp { left, right, .. } => {
                left.collect_live_block_targets(out);
                right.collect_live_block_targets(out);
            }
            RenderExpr::Array { items } => {
                for item in items {
                    item.collect_live_block_targets(out);
                }
            }
            RenderExpr::Object { fields } => {
                for expr in fields.values() {
                    expr.collect_live_block_targets(out);
                }
            }
            RenderExpr::ColumnRef { .. } | RenderExpr::Literal { .. } => {}
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
        RenderExpr::LiveBlock { .. }
        | RenderExpr::ColumnRef { .. }
        | RenderExpr::Literal { .. } => {}
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
    fn to_rhai_list_with_live_block() {
        let expr = fc(
            "list",
            vec![named_arg("item_template", fc("live_block", vec![]))],
        );
        assert_eq!(expr.to_rhai(), r#"list(#{item_template: live_block()})"#);
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
                named_arg("item_template", fc("live_block", vec![])),
            ],
        );
        assert_eq!(
            expr.to_rhai(),
            r#"columns(#{gap: 4, item_template: live_block()})"#,
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
    fn visible_columns_empty_for_live_block() {
        let expr = fc(
            "list",
            vec![named_arg("item_template", fc("live_block", vec![]))],
        );
        assert!(expr.visible_columns().is_empty());
    }
}
