use std::collections::HashMap;
use std::fmt::Write;

use holon_api::render_types::OperationWiring;
use holon_api::streaming::CollectionId;
use holon_api::Value;
use serde::{Deserialize, Serialize};

use crate::input_trigger::InputTrigger;

/// Lazily-expandable children for any node that contains sub-nodes.
///
/// Any widget with children wraps them in `LazyChildren`. For small collections
/// (< initial window), all items are materialized immediately. For large ones,
/// the frontend requests more via `expand_range()` as the user scrolls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LazyChildren {
    pub total_count: usize,
    pub items: Vec<ViewModel>,
    pub offset: usize,
    pub collection_id: Option<CollectionId>,
}

impl LazyChildren {
    /// Create fully-materialized children (no lazy expansion needed).
    pub fn fully_materialized(items: Vec<ViewModel>) -> Self {
        let total_count = items.len();
        Self {
            total_count,
            items,
            offset: 0,
            collection_id: None,
        }
    }

    /// Create a lazy window into a larger collection.
    pub fn lazy(
        total_count: usize,
        items: Vec<ViewModel>,
        offset: usize,
        collection_id: CollectionId,
    ) -> Self {
        Self {
            total_count,
            items,
            offset,
            collection_id: Some(collection_id),
        }
    }

    pub fn is_fully_materialized(&self) -> bool {
        self.collection_id.is_none() || self.items.len() == self.total_count
    }
}

/// A node in the shadow widget tree.
///
/// `kind` describes what kind of widget this is with typed fields.
/// `operations` carries the operation bindings from the RenderExpr — used by
/// `ShadowDom` for input bubbling (e.g., matching key chords to operations).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewModel {
    /// The underlying data row from which this node was constructed. Frontends
    /// can read any property (id, collapse_to, etc.) without the shadow layer
    /// having to forward individual fields.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub entity: HashMap<String, Value>,

    #[serde(flatten)]
    pub kind: NodeKind,

    /// Operations available at this node. Populated by ProfileResolver via
    /// the render_block builder.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<OperationWiring>,

    /// Input triggers for this node. The View checks these locally on every
    /// keystroke and only sends a ViewEvent when a trigger matches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<InputTrigger>,
}

/// The kind of widget this node represents.
///
/// Each variant has typed fields instead of stringly-typed `widget: String`.
/// Variants with children use `LazyChildren` for unified lazy expansion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "widget", rename_all = "snake_case")]
pub enum NodeKind {
    // ── Leaf nodes (no children) ──────────────────────────────────────
    Text {
        content: String,
        #[serde(default)]
        bold: bool,
        #[serde(default = "default_text_size")]
        size: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
    },
    Badge {
        label: String,
    },
    Icon {
        #[serde(default = "default_icon_name")]
        name: String,
        #[serde(default = "default_icon_size")]
        size: f32,
    },
    Checkbox {
        #[serde(default)]
        checked: bool,
    },
    Spacer {
        #[serde(default)]
        width: f32,
        #[serde(default)]
        height: f32,
    },
    EditableText {
        content: String,
        #[serde(default = "default_editable_field")]
        field: String,
    },

    // ── Nodes with lazy children ──────────────────────────────────────
    Row {
        #[serde(default = "default_row_gap")]
        gap: f32,
        children: LazyChildren,
    },
    Block {
        children: LazyChildren,
    },
    Section {
        title: String,
        children: LazyChildren,
    },
    List {
        #[serde(default = "default_list_gap")]
        gap: f32,
        children: LazyChildren,
    },
    Tree {
        children: LazyChildren,
    },
    Outline {
        children: LazyChildren,
    },
    Table {
        children: LazyChildren,
    },
    Columns {
        #[serde(default = "default_columns_gap")]
        gap: f32,
        children: LazyChildren,
    },
    /// Generic column layout (e.g. from Array/Object RenderExpr)
    Col {
        children: LazyChildren,
    },

    // ── Elements (data fields, some with children) ────────────────────
    SourceBlock {
        #[serde(default = "default_language")]
        language: String,
        content: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        editable: bool,
    },
    SourceEditor {
        #[serde(default = "default_language")]
        language: String,
        content: String,
    },
    BlockOperations {
        operations: String,
    },
    StateToggle {
        #[serde(default = "default_task_state_field")]
        field: String,
        current: String,
        #[serde(default)]
        label: String,
        states: String,
    },
    PrefField {
        key: String,
        pref_type: String,
        value: Value,
        #[serde(default)]
        requires_restart: bool,
        #[serde(default)]
        options: Vec<Value>,
        children: LazyChildren,
    },
    QueryResult {
        children: LazyChildren,
    },
    /// A single table row with column data
    TableRow {
        data: HashMap<String, Value>,
    },

    // ── Wrappers (single child passthrough) ───────────────────────────
    Focusable {
        child: Box<ViewModel>,
    },
    Selectable {
        child: Box<ViewModel>,
    },
    Draggable {
        child: Box<ViewModel>,
    },
    PieMenu {
        #[serde(default)]
        fields: String,
        child: Box<ViewModel>,
    },
    DropZone,
    /// A collapsible region (e.g. sidebar). Discovered from `collapse_to`
    /// property on data rows. Frontends render these as hideable panels;
    /// window chrome can walk the tree to find drawers for toggle buttons.
    Drawer {
        block_id: String,
        child: Box<ViewModel>,
    },

    // ── Special ───────────────────────────────────────────────────────
    BlockRef {
        block_id: String,
        content: Box<ViewModel>,
    },
    LiveQuery {
        content: Box<ViewModel>,
        /// Compiled SQL for reactive subscription (compiled from PRQL/GQL/SQL input).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compiled_sql: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query_context_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        render_expr: Option<holon_api::render_types::RenderExpr>,
    },
    RenderBlock {
        content: Box<ViewModel>,
    },
    Error {
        message: String,
    },
    Empty,

    /// A tree item wrapper (node + indented children)
    TreeItem {
        children: LazyChildren,
    },
}

fn default_icon_name() -> String {
    "circle".to_string()
}
fn default_icon_size() -> f32 {
    16.0
}
fn default_text_size() -> f32 {
    14.0
}
fn default_editable_field() -> String {
    "content".to_string()
}
fn default_row_gap() -> f32 {
    8.0
}
fn default_list_gap() -> f32 {
    4.0
}
fn default_columns_gap() -> f32 {
    16.0
}
fn default_language() -> String {
    "text".to_string()
}
fn default_task_state_field() -> String {
    "task_state".to_string()
}

impl Default for ViewModel {
    fn default() -> Self {
        Self {
            entity: HashMap::new(),
            kind: NodeKind::Empty,
            operations: vec![],
            triggers: vec![],
        }
    }
}

impl PartialEq for ViewModel {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

// ---------------------------------------------------------------------------
// Constructors — backward-compatible with old stringly-typed API
// ---------------------------------------------------------------------------

impl ViewModel {
    /// Create a collection node. Maps widget name to typed variant.
    pub fn collection(widget: impl Into<String>, items: Vec<ViewModel>) -> Self {
        let widget = widget.into();
        let children = LazyChildren::fully_materialized(items);
        let kind = match widget.as_str() {
            "list" => NodeKind::List {
                gap: default_list_gap(),
                children,
            },
            "tree" => NodeKind::Tree { children },
            "table" => NodeKind::Table { children },
            "outline" => NodeKind::Outline { children },
            "query_result" => NodeKind::QueryResult { children },
            _ => NodeKind::List {
                gap: default_list_gap(),
                children,
            },
        };
        Self {
            entity: HashMap::new(),
            kind,
            ..Default::default()
        }
    }

    /// Set the entity data for this node (builder pattern).
    pub fn with_entity(mut self, entity: HashMap<String, Value>) -> Self {
        self.entity = entity;
        self
    }

    /// Create a ViewModel from a pre-built NodeKind.
    pub fn from_kind(kind: NodeKind) -> Self {
        Self {
            entity: HashMap::new(),
            kind,
            ..Default::default()
        }
    }

    /// Create a layout node. Maps widget name to typed variant.
    pub fn layout(widget: impl Into<String>, children: Vec<ViewModel>) -> Self {
        let widget = widget.into();
        let lazy = LazyChildren::fully_materialized(children);
        let kind = match widget.as_str() {
            "row" => NodeKind::Row {
                gap: default_row_gap(),
                children: lazy,
            },
            "block" => NodeKind::Block { children: lazy },
            "columns" => NodeKind::Columns {
                gap: default_columns_gap(),
                children: lazy,
            },
            "col" => NodeKind::Col { children: lazy },
            "section" => NodeKind::Section {
                title: String::new(),
                children: lazy,
            },
            "tree_item" => NodeKind::TreeItem { children: lazy },
            _ => NodeKind::Col { children: lazy },
        };
        Self {
            entity: HashMap::new(),
            kind,
            ..Default::default()
        }
    }

    /// Create an element node. Maps widget name to typed variant.
    pub fn element(
        widget: impl Into<String>,
        data: HashMap<String, Value>,
        children: Vec<ViewModel>,
    ) -> Self {
        let widget = widget.into();
        let kind = match widget.as_str() {
            "source_block" => NodeKind::SourceBlock {
                language: data
                    .get("language")
                    .and_then(|v| v.as_string())
                    .unwrap_or("text")
                    .to_string(),
                content: data
                    .get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
                name: data
                    .get("name")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
                editable: data
                    .get("editable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            },
            "source_editor" => NodeKind::SourceEditor {
                language: data
                    .get("language")
                    .and_then(|v| v.as_string())
                    .unwrap_or("text")
                    .to_string(),
                content: data
                    .get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
            },
            "block_operations" => NodeKind::BlockOperations {
                operations: data
                    .get("operations")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
            },
            "state_toggle" => NodeKind::StateToggle {
                field: data
                    .get("field")
                    .and_then(|v| v.as_string())
                    .unwrap_or("task_state")
                    .to_string(),
                current: data
                    .get("current")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
                label: data
                    .get("label")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
                states: data
                    .get("states")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
            },
            "pref_field" => NodeKind::PrefField {
                key: data
                    .get("key")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
                pref_type: data
                    .get("pref_type")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
                value: data.get("value").cloned().unwrap_or(Value::Null),
                requires_restart: data
                    .get("requires_restart")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                options: match data.get("options") {
                    Some(Value::Array(arr)) => arr.clone(),
                    _ => vec![],
                },
                children: LazyChildren::fully_materialized(children),
            },
            "table_row" | "row" => NodeKind::TableRow { data },
            _ => NodeKind::TableRow { data },
        };
        Self {
            entity: HashMap::new(),
            kind,
            ..Default::default()
        }
    }

    /// Create a leaf node. Maps widget name to typed variant.
    pub fn leaf(widget: impl Into<String>, value: Value) -> Self {
        let widget = widget.into();
        let kind = match widget.as_str() {
            "text" => NodeKind::Text {
                content: value.to_display_string(),
                bold: false,
                size: default_text_size(),
                color: None,
            },
            "badge" => NodeKind::Badge {
                label: value.to_display_string(),
            },
            "icon" => NodeKind::Icon {
                name: value.as_string().unwrap_or("circle").to_string(),
                size: default_icon_size(),
            },
            "checkbox" => NodeKind::Checkbox {
                checked: value.as_bool().unwrap_or(false),
            },
            "editable_text" => NodeKind::EditableText {
                content: value.to_display_string(),
                field: default_editable_field(),
            },
            _ => NodeKind::Text {
                content: value.to_display_string(),
                bold: false,
                size: default_text_size(),
                color: None,
            },
        };
        Self {
            entity: HashMap::new(),
            kind,
            ..Default::default()
        }
    }

    pub fn block_ref(block_id: impl Into<String>, content: ViewModel) -> Self {
        Self {
            kind: NodeKind::BlockRef {
                block_id: block_id.into(),
                content: Box::new(content),
            },
            ..Default::default()
        }
    }

    pub fn drawer(block_id: impl Into<String>, child: ViewModel) -> Self {
        Self {
            kind: NodeKind::Drawer {
                block_id: block_id.into(),
                child: Box::new(child),
            },
            ..Default::default()
        }
    }

    pub fn error(_widget: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: NodeKind::Error {
                message: message.into(),
            },
            ..Default::default()
        }
    }

    pub fn empty() -> Self {
        Self {
            kind: NodeKind::Empty,
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Tree traversal and display
// ---------------------------------------------------------------------------

impl ViewModel {
    pub fn pretty_print(&self, indent: usize) -> String {
        let mut out = String::new();
        self.fmt_indent(&mut out, indent);
        out
    }

    fn fmt_indent(&self, out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent);
        let ops_suffix = if self.operations.is_empty() {
            String::new()
        } else {
            format!(
                " [ops: {}]",
                self.operations
                    .iter()
                    .map(|o| o.descriptor.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        match &self.kind {
            // Leaf nodes
            NodeKind::Text { content, bold, .. } => {
                let bold_marker = if *bold { " (bold)" } else { "" };
                let _ = writeln!(out, "{pad}text {content:?}{bold_marker}{ops_suffix}");
            }
            NodeKind::Badge { label } => {
                let _ = writeln!(out, "{pad}badge {label:?}{ops_suffix}");
            }
            NodeKind::Icon { name, .. } => {
                let _ = writeln!(out, "{pad}icon {name:?}{ops_suffix}");
            }
            NodeKind::Checkbox { checked } => {
                let _ = writeln!(out, "{pad}checkbox({checked}){ops_suffix}");
            }
            NodeKind::Spacer { .. } => {
                let _ = writeln!(out, "{pad}spacer{ops_suffix}");
            }
            NodeKind::EditableText { content, .. } => {
                let _ = writeln!(out, "{pad}editable_text {content:?}{ops_suffix}");
            }

            // Nodes with lazy children
            NodeKind::List { children, .. }
            | NodeKind::Tree { children }
            | NodeKind::Table { children }
            | NodeKind::Outline { children }
            | NodeKind::QueryResult { children } => {
                let name = self.widget_name().unwrap_or("collection");
                let _ = writeln!(
                    out,
                    "{pad}{name} [{} items]{ops_suffix}",
                    children.total_count
                );
                for item in &children.items {
                    item.fmt_indent(out, indent + 1);
                }
            }
            NodeKind::Row { children, .. }
            | NodeKind::Block { children }
            | NodeKind::Columns { children, .. }
            | NodeKind::Col { children }
            | NodeKind::TreeItem { children } => {
                let name = self.widget_name().unwrap_or("layout");
                let _ = writeln!(out, "{pad}{name}{ops_suffix}");
                for child in &children.items {
                    child.fmt_indent(out, indent + 1);
                }
            }
            NodeKind::Section { title, children } => {
                let _ = writeln!(out, "{pad}section({title:?}){ops_suffix}");
                for child in &children.items {
                    child.fmt_indent(out, indent + 1);
                }
            }

            // Elements
            NodeKind::SourceBlock {
                language, content, ..
            } => {
                let _ = writeln!(
                    out,
                    "{pad}source_block({language}) {}{ops_suffix}",
                    &content[..content.len().min(40)]
                );
            }
            NodeKind::SourceEditor {
                language, content, ..
            } => {
                let _ = writeln!(
                    out,
                    "{pad}source_editor({language}) {}{ops_suffix}",
                    &content[..content.len().min(40)]
                );
            }
            NodeKind::BlockOperations { operations } => {
                let _ = writeln!(out, "{pad}block_operations({operations}){ops_suffix}");
            }
            NodeKind::StateToggle {
                field,
                current,
                label,
                ..
            } => {
                let _ = writeln!(
                    out,
                    "{pad}state_toggle({field}={current}, {label}){ops_suffix}"
                );
            }
            NodeKind::PrefField {
                key,
                pref_type,
                children,
                ..
            } => {
                let _ = writeln!(out, "{pad}pref_field({key}: {pref_type}){ops_suffix}");
                for child in &children.items {
                    child.fmt_indent(out, indent + 1);
                }
            }
            NodeKind::TableRow { data } => {
                let fields = format_data_inline(data);
                let _ = writeln!(out, "{pad}table_row {{{fields}}}{ops_suffix}");
            }

            // Wrappers
            NodeKind::Focusable { child } => {
                let _ = writeln!(out, "{pad}focusable{ops_suffix}");
                child.fmt_indent(out, indent + 1);
            }
            NodeKind::Selectable { child } => {
                let _ = writeln!(out, "{pad}selectable{ops_suffix}");
                child.fmt_indent(out, indent + 1);
            }
            NodeKind::Draggable { child } => {
                let _ = writeln!(out, "{pad}draggable{ops_suffix}");
                child.fmt_indent(out, indent + 1);
            }
            NodeKind::PieMenu { child, .. } => {
                let _ = writeln!(out, "{pad}pie_menu{ops_suffix}");
                child.fmt_indent(out, indent + 1);
            }
            NodeKind::DropZone => {
                let _ = writeln!(out, "{pad}drop_zone{ops_suffix}");
            }
            NodeKind::Drawer { block_id, child } => {
                let _ = writeln!(out, "{pad}drawer({block_id}){ops_suffix}");
                child.fmt_indent(out, indent + 1);
            }

            // Special
            NodeKind::BlockRef { block_id, content } => {
                let _ = writeln!(out, "{pad}block_ref({block_id}){ops_suffix}");
                content.fmt_indent(out, indent + 1);
            }
            NodeKind::LiveQuery { content, .. } => {
                let _ = writeln!(out, "{pad}live_query{ops_suffix}");
                content.fmt_indent(out, indent + 1);
            }
            NodeKind::RenderBlock { content } => {
                let _ = writeln!(out, "{pad}render_block{ops_suffix}");
                content.fmt_indent(out, indent + 1);
            }
            NodeKind::Error { message } => {
                let _ = writeln!(out, "{pad}ERROR: {message}");
            }
            NodeKind::Empty => {
                let _ = writeln!(out, "{pad}(empty)");
            }
        }
    }

    /// Collect drawer block IDs from the tree, in depth-first order.
    pub fn collect_drawer_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        self.collect_drawers_recursive(&mut ids);
        ids
    }

    fn collect_drawers_recursive(&self, ids: &mut Vec<String>) {
        if let NodeKind::Drawer { block_id, .. } = &self.kind {
            ids.push(block_id.clone());
        }
        for child in self.children() {
            child.collect_drawers_recursive(ids);
        }
    }

    /// Collect all entity IDs referenced in the tree, in depth-first order.
    pub fn collect_entity_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        self.collect_ids_recursive(&mut ids);
        ids
    }

    fn collect_ids_recursive(&self, ids: &mut Vec<String>) {
        match &self.kind {
            NodeKind::BlockRef { block_id, content } => {
                ids.push(block_id.clone());
                content.collect_ids_recursive(ids);
            }
            NodeKind::TableRow { data } => {
                if let Some(id) = data.get("id").and_then(|v| v.as_string()) {
                    ids.push(id.to_string());
                }
            }
            _ => {
                for child in self.children() {
                    child.collect_ids_recursive(ids);
                }
            }
        }
    }

    /// Get children of this node as a slice.
    pub fn children(&self) -> &[ViewModel] {
        match &self.kind {
            // LazyChildren nodes
            NodeKind::Row { children, .. }
            | NodeKind::Block { children }
            | NodeKind::Section { children, .. }
            | NodeKind::List { children, .. }
            | NodeKind::Tree { children }
            | NodeKind::Outline { children }
            | NodeKind::Table { children }
            | NodeKind::Columns { children, .. }
            | NodeKind::Col { children }
            | NodeKind::QueryResult { children }
            | NodeKind::PrefField { children, .. }
            | NodeKind::TreeItem { children } => &children.items,

            // Box<ViewModel> wrappers
            NodeKind::Focusable { child }
            | NodeKind::Selectable { child }
            | NodeKind::Draggable { child }
            | NodeKind::Drawer { child, .. }
            | NodeKind::PieMenu { child, .. }
            | NodeKind::BlockRef { content: child, .. }
            | NodeKind::LiveQuery { content: child, .. }
            | NodeKind::RenderBlock { content: child } => std::slice::from_ref(child.as_ref()),

            // Leaf nodes
            NodeKind::Text { .. }
            | NodeKind::Badge { .. }
            | NodeKind::Icon { .. }
            | NodeKind::Checkbox { .. }
            | NodeKind::Spacer { .. }
            | NodeKind::EditableText { .. }
            | NodeKind::SourceBlock { .. }
            | NodeKind::SourceEditor { .. }
            | NodeKind::BlockOperations { .. }
            | NodeKind::StateToggle { .. }
            | NodeKind::TableRow { .. }
            | NodeKind::DropZone
            | NodeKind::Error { .. }
            | NodeKind::Empty => &[],
        }
    }

    /// The widget name (e.g. "list", "row", "text").
    pub fn widget_name(&self) -> Option<&str> {
        Some(match &self.kind {
            NodeKind::Text { .. } => "text",
            NodeKind::Badge { .. } => "badge",
            NodeKind::Icon { .. } => "icon",
            NodeKind::Checkbox { .. } => "checkbox",
            NodeKind::Spacer { .. } => "spacer",
            NodeKind::EditableText { .. } => "editable_text",
            NodeKind::Row { .. } => "row",
            NodeKind::Block { .. } => "block",
            NodeKind::Section { .. } => "section",
            NodeKind::List { .. } => "list",
            NodeKind::Tree { .. } => "tree",
            NodeKind::Outline { .. } => "outline",
            NodeKind::Table { .. } => "table",
            NodeKind::Columns { .. } => "columns",
            NodeKind::Col { .. } => "col",
            NodeKind::SourceBlock { .. } => "source_block",
            NodeKind::SourceEditor { .. } => "source_editor",
            NodeKind::BlockOperations { .. } => "block_operations",
            NodeKind::StateToggle { .. } => "state_toggle",
            NodeKind::PrefField { .. } => "pref_field",
            NodeKind::QueryResult { .. } => "query_result",
            NodeKind::TableRow { .. } => "table_row",
            NodeKind::Focusable { .. } => "focusable",
            NodeKind::Selectable { .. } => "selectable",
            NodeKind::Draggable { .. } => "draggable",
            NodeKind::PieMenu { .. } => "pie_menu",
            NodeKind::DropZone => "drop_zone",
            NodeKind::Drawer { .. } => "drawer",
            NodeKind::BlockRef { .. } => "block_ref",
            NodeKind::LiveQuery { .. } => "live_query",
            NodeKind::RenderBlock { .. } => "render_block",
            NodeKind::Error { .. } => "error",
            NodeKind::TreeItem { .. } => "tree_item",
            NodeKind::Empty => return None,
        })
    }

    /// Extract entity ID from element data or BlockRef block_id.
    pub fn entity_id(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::TableRow { data } => data.get("id").and_then(|v| v.as_string()),
            NodeKind::BlockRef { block_id, .. } => Some(block_id.as_str()),
            _ => None,
        }
    }
}

fn format_data_inline(data: &HashMap<String, Value>) -> String {
    let mut pairs: Vec<_> = data
        .iter()
        .filter(|(k, _)| *k == "id" || *k == "content" || *k == "task_state")
        .collect();
    pairs.sort_by_key(|(k, _)| *k);
    pairs
        .iter()
        .map(|(k, v)| format!("{k}: {:?}", v.to_display_string()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_print_nested() {
        let tree = ViewModel::layout(
            "columns",
            vec![ViewModel::collection(
                "list",
                vec![
                    ViewModel::block_ref(
                        "a",
                        ViewModel::element(
                            "table_row",
                            HashMap::from([
                                ("id".into(), Value::String("a".into())),
                                ("content".into(), Value::String("First".into())),
                            ]),
                            vec![],
                        ),
                    ),
                    ViewModel::block_ref(
                        "b",
                        ViewModel::element(
                            "table_row",
                            HashMap::from([
                                ("id".into(), Value::String("b".into())),
                                ("content".into(), Value::String("Second".into())),
                            ]),
                            vec![],
                        ),
                    ),
                ],
            )],
        );

        let output = tree.pretty_print(0);
        assert!(output.contains("columns"));
        assert!(output.contains("list [2 items]"));
        assert!(output.contains("block_ref(a)"));
        assert!(output.contains("block_ref(b)"));
    }

    #[test]
    fn collect_entity_ids_mixed() {
        let tree = ViewModel::layout(
            "col",
            vec![
                ViewModel::block_ref(
                    "ref-1",
                    ViewModel::element(
                        "table_row",
                        HashMap::from([("id".into(), Value::String("inner-1".into()))]),
                        vec![],
                    ),
                ),
                ViewModel::element(
                    "table_row",
                    HashMap::from([("id".into(), Value::String("row-1".into()))]),
                    vec![],
                ),
            ],
        );

        let ids = tree.collect_entity_ids();
        assert_eq!(ids, vec!["ref-1", "inner-1", "row-1"]);
    }

    #[test]
    fn children_accessor() {
        let list = ViewModel::collection("list", vec![ViewModel::empty(), ViewModel::empty()]);
        assert_eq!(list.children().len(), 2);

        let leaf = ViewModel::leaf("text", Value::String("hi".into()));
        assert!(leaf.children().is_empty());
    }

    #[test]
    fn entity_id_extraction() {
        let elem = ViewModel::element(
            "table_row",
            HashMap::from([("id".into(), Value::String("abc".into()))]),
            vec![],
        );
        assert_eq!(elem.entity_id(), Some("abc"));

        let bref = ViewModel::block_ref("xyz", ViewModel::empty());
        assert_eq!(bref.entity_id(), Some("xyz"));

        assert_eq!(ViewModel::empty().entity_id(), None);
    }

    #[test]
    fn lazy_children_fully_materialized() {
        let lc = LazyChildren::fully_materialized(vec![ViewModel::empty(), ViewModel::empty()]);
        assert_eq!(lc.total_count, 2);
        assert_eq!(lc.items.len(), 2);
        assert!(lc.is_fully_materialized());
    }

    #[test]
    fn lazy_children_lazy() {
        let lc = LazyChildren::lazy(100, vec![ViewModel::empty(); 20], 0, CollectionId(42));
        assert_eq!(lc.total_count, 100);
        assert_eq!(lc.items.len(), 20);
        assert!(!lc.is_fully_materialized());
    }
}
