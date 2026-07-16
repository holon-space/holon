use std::collections::HashMap;
use std::fmt::Write;
use std::str::FromStr;
use std::sync::Arc;

use holon_api::render_types::OperationWiring;
use holon_api::widget_spec::DataRow;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Value;
use serde::Deserialize;
use serde::Serialize;

use crate::input_trigger::InputTrigger;
use crate::render_context::LayoutHint;

fn is_default_layout_hint(h: &LayoutHint) -> bool {
    *h == LayoutHint::default()
}

fn arc_map_is_empty(m: &Arc<DataRow>) -> bool {
    m.is_empty()
}

/// Children container for snapshot `ViewModel` nodes.
///
/// All children are fully materialized — lazy expansion is handled at the
/// reactive layer (`ReactiveCollection`) via `MutableVec` + `VecDiff`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LazyChildren {
    pub items: Vec<ViewModel>,
}

impl LazyChildren {
    pub fn fully_materialized(items: Vec<ViewModel>) -> Self {
        Self { items }
    }
}

/// Wire envelope for snapshot-pipeline watch emissions (wasm worker →
/// web page). Carries the in-memory editor-focus state (ADR 0010)
/// atomically with the `ViewModel` interpretation that consumed it, so
/// the page can move DOM focus and seed the caret without a second,
/// racy RPC. Produced by the worker's `engine_watch_view` callback from
/// `ReactiveEngine::watch_snapshot_stream` emissions; parsed by
/// `holon-dioxus-web`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchEnvelope {
    pub view_model: ViewModel,
    /// The focused block's entity URI, if any (`UiState::focused_block`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_block: Option<String>,
    /// One-shot initial caret offset armed for the focused block
    /// (`UiState::peek_caret_seed`) — split/join/nav placement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caret_offset: Option<usize>,
}

/// How a drawer behaves relative to sibling layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawerMode {
    /// The drawer shrinks sibling content when open (default sidebar
    /// behaviour). Width = sidebar_width when open, 0 when closed.
    #[default]
    Shrink,
    /// The drawer floats over sibling content without affecting their size.
    /// Siblings always receive the full available width; the drawer is rendered
    /// in a stacking layer above them. Typical for narrow (phone) layouts.
    Overlay,
}

impl DrawerMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "overlay" => Self::Overlay,
            _ => Self::Shrink,
        }
    }

    /// Whether a drawer of this mode starts open when the user has no stored
    /// preference. `Shrink` drawers (desktop sidebars that reserve width) start
    /// open; `Overlay` drawers (narrow/phone layouts that float over the main
    /// panel) start closed — an open-by-default overlay would obscure the
    /// content on first paint.
    pub fn default_open(self) -> bool {
        matches!(self, DrawerMode::Shrink)
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
    /// having to forward individual fields. Arc-wrapped for cheap cloning
    /// through the reactive pipeline.
    #[serde(default, skip_serializing_if = "arc_map_is_empty")]
    pub entity: Arc<DataRow>,

    #[serde(flatten)]
    pub kind: ViewKind,

    /// Operations available at this node. Populated by ProfileResolver via
    /// the render_entity builder.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<OperationWiring>,

    /// Input triggers for this node. The View checks these locally on every
    /// keystroke and only sends a ViewEvent when a trigger matches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<InputTrigger>,

    /// Hint to the parent layout container about how much space this node
    /// needs. `Flex { weight: 1 }` is the default (equal share of remaining
    /// space). `Fixed { px }` claims an exact number of pixels.
    #[serde(default, skip_serializing_if = "is_default_layout_hint")]
    pub layout_hint: LayoutHint,

    /// Which display occurrence of the row this node renders (ADR 0015 rule 4:
    /// node metadata, NEVER an id-infix). `Canonical` for every real row;
    /// `ReactiveViewModel::snapshot` threads the stamped `Placed` occurrence
    /// through so PBT snapshots can observe display-placed rows without the
    /// occurrence ever touching the `EntityUri`. `#[serde(skip)]`: the
    /// coordinate is in-memory only — it never rides the serialized wire
    /// form, keeping the ViewModel bytes identical to before this widening.
    #[serde(skip)]
    pub occurrence: holon_api::Occurrence,
}

/// The kind of widget this node represents.
///
/// Each variant has typed fields instead of stringly-typed `widget: String`.
/// Variants with children use `LazyChildren` for unified lazy expansion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "widget", rename_all = "snake_case")]
pub enum ViewKind {
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
    /// A horizontal separator line (LogSeq-style, between Journal-feed entries).
    /// A leaf with no fields; the frontend divider builder draws the rule.
    Divider,
    Spacer {
        #[serde(default)]
        width: f32,
        #[serde(default)]
        height: f32,
        #[serde(default)]
        color: Option<String>,
    },
    EditableText {
        content: String,
        #[serde(default = "default_editable_field")]
        field: String,
    },
    /// Read-only sibling of `EditableText`. Renders the same content but
    /// does not mount a live editor — click dispatches
    /// `navigation.editor_focus` which flips the block_profile variant to
    /// `is_focused` and swaps in `editable_text` on the next render.
    RenderedText {
        content: String,
        #[serde(default = "default_editable_field")]
        field: String,
    },
    /// Inline image loaded from a file path.
    Image {
        path: String,
        #[serde(default)]
        alt: String,
        #[serde(default)]
        width: Option<f32>,
        #[serde(default)]
        height: Option<f32>,
    },

    // ── Nodes with lazy children ──────────────────────────────────────
    Row {
        #[serde(default = "default_row_gap")]
        gap: f32,
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
    /// Kanban board: rows grouped into lanes by `lane_field`.
    ///
    /// Snapshot fidelity limit (deliberate): `children` are the board's rows
    /// FLAT — lanes are not modeled as nested structure; `lane_field` is
    /// carried so snapshot consumers can group. Live GPUI lane rendering
    /// reads the `ReactiveViewModel` collection directly and never sees this.
    Board {
        lane_field: String,
        children: LazyChildren,
    },
    /// Generic column layout (e.g. from Array/Object RenderExpr)
    Column {
        #[serde(default)]
        gap: f32,
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
    ExpandToggle {
        target_id: String,
        expanded: bool,
        children: LazyChildren,
    },
    PrefField {
        key: String,
        pref_type: String,
        value: Value,
        #[serde(default)]
        requires_restart: bool,
        #[serde(default)]
        locked: bool,
        #[serde(default)]
        options: Vec<Value>,
        children: LazyChildren,
    },
    QueryResult {
        children: LazyChildren,
    },
    /// A single table row with column data
    TableRow {
        data: Arc<DataRow>,
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
    /// A drop target. `op_name` is the operation dispatched when a drag is
    /// released on this zone — defaults to `move_block`. Production GPUI
    /// `drop_zone.rs` and the headless `UserDriver::drop_entity` both read
    /// this to build the dispatched intent (see `build_drop_intent`).
    DropZone {
        #[serde(default = "default_drop_op_name")]
        op_name: String,
    },
    /// View mode switcher: ghost icons for switching collection layout.
    /// `entity_uri` identifies which collection this switcher controls.
    ViewModeSwitcher {
        entity_uri: holon_api::EntityUri,
        modes: String,
        child: Box<ViewModel>,
    },
    /// A collapsible region (e.g. sidebar). Discovered from `collapse_to`
    /// property on data rows. Frontends render these as hideable panels;
    /// window chrome can walk the tree to find drawers for toggle buttons.
    Drawer {
        block_id: String,
        mode: DrawerMode,
        /// Reserved width in logical pixels. Used by the parent columns()
        /// builder (via layout_hint) to compute how much space to allocate.
        width: f32,
        child: Box<ViewModel>,
    },
    /// Accent card — tinted background + colored left border.
    Card {
        accent: String,
        children: LazyChildren,
    },
    ChatBubble {
        sender: String,
        time: String,
        children: LazyChildren,
    },
    Collapsible {
        header: String,
        icon: String,
        children: LazyChildren,
    },
    /// Hover-reveal container. `children.items[0]` is the always-visible
    /// trigger; `children.items[1..]` is the content shown only while the
    /// trigger's region is hovered. Hover state is per-render-slot view
    /// state, not carried in the snapshot — each frontend owns its own
    /// hover signal seeded to "not hovered".
    OnHover {
        children: LazyChildren,
    },
    /// Two-slot anchored container. `children.items[0]` fills the remaining
    /// vertical space; `children.items[1]` is pinned at its intrinsic height
    /// anchored to the bottom inset (IME / nav bar / home indicator). GPUI
    /// anchors the dock against `safe_area_bottom_px()`; snapshot consumers
    /// see two slots with no platform-specific positioning. The builder
    /// enforces exactly two slots at construction time.
    BottomDock {
        children: LazyChildren,
    },
    /// Single-purpose tappable affordance for invoking an operation.
    ///
    /// Used by the mobile action bar (`row(#{ collection: chain_ops(0),
    /// item_template: op_button(col("name")) })`). On tap the platform
    /// renderer resolves the `OperationDescriptor` matching `op_name` on
    /// the profile for `target_id`, and dispatches via
    /// `BuilderServices::present_op`. `display_name` is the a11y label
    /// and the alternate short-label for sighted users; `icon` is the
    /// op_name passed through so GPUI can apply its hardcoded icon table.
    OpButton {
        op_name: String,
        target_id: String,
        display_name: String,
    },

    // ── Special ───────────────────────────────────────────────────────
    LiveBlock {
        block_id: String,
        content: Box<ViewModel>,
    },
    LiveQuery {
        content: Box<ViewModel>,
        /// Source query text (PRQL/GQL/SQL) for reactive subscription —
        /// compiled behind the query capability when the platform subscribes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query: Option<String>,
        /// Language of `query` (`holon_prql`/`holon_gql`/`holon_sql`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query_lang: Option<String>,
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
    /// Stream hasn't delivered a structure event yet. Renders as nothing.
    Loading,

    /// A flat tree item: single content child + depth metadata for indentation.
    TreeItem {
        depth: usize,
        has_children: bool,
        children: LazyChildren,
    },
}

impl ViewKind {
    /// Stable string tag for the variant. Matches the serde `widget` tag
    /// (snake_case). Used by logging and diagnostics instead of scraping
    /// `Debug` output.
    pub fn tag(&self) -> &'static str {
        match self {
            ViewKind::Text { .. } => "text",
            ViewKind::Badge { .. } => "badge",
            ViewKind::Icon { .. } => "icon",
            ViewKind::Checkbox { .. } => "checkbox",
            ViewKind::Divider => "divider",
            ViewKind::Spacer { .. } => "spacer",
            ViewKind::EditableText { .. } => "editable_text",
            ViewKind::RenderedText { .. } => "rendered_text",
            ViewKind::Image { .. } => "image",
            ViewKind::Row { .. } => "row",
            ViewKind::Section { .. } => "section",
            ViewKind::List { .. } => "list",
            ViewKind::Tree { .. } => "tree",
            ViewKind::Outline { .. } => "outline",
            ViewKind::Table { .. } => "table",
            ViewKind::Columns { .. } => "columns",
            ViewKind::Board { .. } => "board",
            ViewKind::Column { .. } => "column",
            ViewKind::SourceBlock { .. } => "source_block",
            ViewKind::SourceEditor { .. } => "source_editor",
            ViewKind::BlockOperations { .. } => "block_operations",
            ViewKind::StateToggle { .. } => "state_toggle",
            ViewKind::ExpandToggle { .. } => "expand_toggle",
            ViewKind::PrefField { .. } => "pref_field",
            ViewKind::QueryResult { .. } => "query_result",
            ViewKind::TableRow { .. } => "table_row",
            ViewKind::Focusable { .. } => "focusable",
            ViewKind::Selectable { .. } => "selectable",
            ViewKind::Draggable { .. } => "draggable",
            ViewKind::PieMenu { .. } => "pie_menu",
            ViewKind::DropZone { .. } => "drop_zone",
            ViewKind::ViewModeSwitcher { .. } => "view_mode_switcher",
            ViewKind::Drawer { .. } => "drawer",
            ViewKind::Card { .. } => "card",
            ViewKind::ChatBubble { .. } => "chat_bubble",
            ViewKind::Collapsible { .. } => "collapsible",
            ViewKind::OnHover { .. } => "on_hover",
            ViewKind::BottomDock { .. } => "bottom_dock",
            ViewKind::OpButton { .. } => "op_button",
            ViewKind::LiveBlock { .. } => "live_block",
            ViewKind::LiveQuery { .. } => "live_query",
            ViewKind::RenderBlock { .. } => "render_entity",
            ViewKind::Error { .. } => "error",
            ViewKind::Empty => "empty",
            ViewKind::Loading => "loading",
            ViewKind::TreeItem { .. } => "tree_item",
        }
    }
}

fn default_drop_op_name() -> String {
    "move_block".to_string()
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

// ---------------------------------------------------------------------------
// Widget-kind enums — parse the builder-supplied widget name into a typed,
// closed vocabulary at the `ViewModel` constructor boundary. Every string that
// reaches these constructors is a builder-controlled literal (the render DSL
// dispatches its function names through a builder registry first), so an
// unknown name is a programmer error, not untrusted input — hence parsing is
// fail-loud (panic) rather than the old silent `_ => <default>`. The
// exhaustive match below also makes adding a `ViewKind` variant a compile
// error until each constructor handles it.
// ---------------------------------------------------------------------------

/// Parse a builder widget name into a typed kind, failing loud on an unknown
/// name (a builder bug, never untrusted input).
fn parse_widget<T: FromStr>(category: &str, allowed: &str, widget: &str) -> T {
    widget.parse().unwrap_or_else(|_| {
        panic!("unknown {category} widget {widget:?}; expected one of {allowed}")
    })
}

/// Widget names accepted by [`ViewModel::collection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionWidget {
    List,
    Tree,
    Table,
    Outline,
    QueryResult,
}

impl FromStr for CollectionWidget {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(match s {
            "list" => Self::List,
            "tree" => Self::Tree,
            "table" => Self::Table,
            "outline" => Self::Outline,
            "query_result" => Self::QueryResult,
            _ => return Err(()),
        })
    }
}

/// Widget names accepted by [`ViewModel::layout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutWidget {
    Row,
    Columns,
    Column,
    Section,
    TreeItem,
    Card,
    ChatBubble,
    Collapsible,
    ExpandToggle,
    BottomDock,
}

impl FromStr for LayoutWidget {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(match s {
            "row" => Self::Row,
            "columns" => Self::Columns,
            "column" => Self::Column,
            "section" => Self::Section,
            "tree_item" => Self::TreeItem,
            "card" => Self::Card,
            "chat_bubble" => Self::ChatBubble,
            "collapsible" => Self::Collapsible,
            "expand_toggle" => Self::ExpandToggle,
            "bottom_dock" => Self::BottomDock,
            _ => return Err(()),
        })
    }
}

/// Widget names accepted by [`ViewModel::element`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementWidget {
    SourceBlock,
    SourceEditor,
    BlockOperations,
    StateToggle,
    ExpandToggle,
    PrefField,
    TableRow,
}

impl FromStr for ElementWidget {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(match s {
            "source_block" => Self::SourceBlock,
            "source_editor" => Self::SourceEditor,
            "block_operations" => Self::BlockOperations,
            "state_toggle" => Self::StateToggle,
            "expand_toggle" => Self::ExpandToggle,
            "pref_field" => Self::PrefField,
            "table_row" | "row" => Self::TableRow,
            _ => return Err(()),
        })
    }
}

/// Widget names accepted by [`ViewModel::leaf`]. `Dropdown`/`SecretText`/
/// `PlatformAction` are preference-field leaves the snapshot tree has no rich
/// variant for; they render as plain text here while the live
/// `ReactiveViewModel` path renders them properly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafWidget {
    Text,
    Badge,
    Icon,
    Checkbox,
    EditableText,
    RenderedText,
    Dropdown,
    SecretText,
    PlatformAction,
}

impl FromStr for LeafWidget {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(match s {
            "text" => Self::Text,
            "badge" => Self::Badge,
            "icon" => Self::Icon,
            "checkbox" => Self::Checkbox,
            "editable_text" => Self::EditableText,
            "rendered_text" => Self::RenderedText,
            "dropdown" => Self::Dropdown,
            "secret_text" => Self::SecretText,
            "platform_action" => Self::PlatformAction,
            _ => return Err(()),
        })
    }
}

impl Default for ViewModel {
    fn default() -> Self {
        Self {
            entity: Arc::new(HashMap::new()),
            kind: ViewKind::Empty,
            operations: vec![],
            triggers: vec![],
            layout_hint: LayoutHint::default(),
            occurrence: holon_api::Occurrence::Canonical,
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
        let kind = match parse_widget::<CollectionWidget>(
            "collection",
            "list|tree|table|outline|query_result",
            &widget,
        ) {
            CollectionWidget::List => ViewKind::List {
                gap: default_list_gap(),
                children,
            },
            CollectionWidget::Tree => ViewKind::Tree { children },
            CollectionWidget::Table => ViewKind::Table { children },
            CollectionWidget::Outline => ViewKind::Outline { children },
            CollectionWidget::QueryResult => ViewKind::QueryResult { children },
        };
        Self {
            entity: Arc::new(HashMap::new()),
            kind,
            ..Default::default()
        }
    }

    /// Set the entity data for this node (builder pattern).
    pub fn with_entity(mut self, entity: Arc<DataRow>) -> Self {
        self.entity = entity;
        self
    }
}

impl crate::render_interpreter::WithEntity for ViewModel {
    fn attach_entity(&mut self, entity: Arc<DataRow>) {
        self.entity = entity;
    }
}

impl ViewModel {
    /// Create a ViewModel from a pre-built ViewKind.
    pub fn from_kind(kind: ViewKind) -> Self {
        Self {
            entity: Arc::new(HashMap::new()),
            kind,
            ..Default::default()
        }
    }

    /// Create a layout node. Maps widget name to typed variant.
    pub fn layout(widget: impl Into<String>, children: Vec<ViewModel>) -> Self {
        let widget = widget.into();
        let lazy = LazyChildren::fully_materialized(children);
        let kind = match parse_widget::<LayoutWidget>(
            "layout",
            "row|columns|column|section|tree_item|card|chat_bubble|collapsible|expand_toggle|bottom_dock",
            &widget,
        ) {
            LayoutWidget::Row => ViewKind::Row {
                gap: default_row_gap(),
                children: lazy,
            },
            LayoutWidget::Columns => ViewKind::Columns {
                gap: default_columns_gap(),
                children: lazy,
            },
            LayoutWidget::Column => ViewKind::Column {
                gap: 0.0,
                children: lazy,
            },
            LayoutWidget::Section => ViewKind::Section {
                title: String::new(),
                children: lazy,
            },
            LayoutWidget::TreeItem => ViewKind::TreeItem {
                depth: 0,
                has_children: false,
                children: lazy,
            },
            LayoutWidget::Card => ViewKind::Card {
                accent: String::new(),
                children: lazy,
            },
            LayoutWidget::ChatBubble => ViewKind::ChatBubble {
                sender: String::new(),
                time: String::new(),
                children: lazy,
            },
            LayoutWidget::Collapsible => ViewKind::Collapsible {
                header: String::new(),
                icon: String::new(),
                children: lazy,
            },
            LayoutWidget::ExpandToggle => ViewKind::ExpandToggle {
                target_id: String::new(),
                expanded: false,
                children: lazy,
            },
            LayoutWidget::BottomDock => {
                assert_eq!(
                    lazy.items.len(),
                    2,
                    "bottom_dock requires exactly 2 slots (main, dock); got {}",
                    lazy.items.len()
                );
                ViewKind::BottomDock { children: lazy }
            }
        };
        Self {
            entity: Arc::new(HashMap::new()),
            kind,
            ..Default::default()
        }
    }

    /// Create an element node. Maps widget name to typed variant.
    pub fn element(
        widget: impl Into<String>,
        data: Arc<DataRow>,
        children: Vec<ViewModel>,
    ) -> Self {
        let widget = widget.into();
        let kind = match parse_widget::<ElementWidget>(
            "element",
            "source_block|source_editor|block_operations|state_toggle|expand_toggle|pref_field|table_row|row",
            &widget,
        ) {
            ElementWidget::SourceBlock => ViewKind::SourceBlock {
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
            ElementWidget::SourceEditor => ViewKind::SourceEditor {
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
            ElementWidget::BlockOperations => ViewKind::BlockOperations {
                operations: data
                    .get("operations")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
            },
            ElementWidget::StateToggle => ViewKind::StateToggle {
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
            ElementWidget::ExpandToggle => ViewKind::ExpandToggle {
                target_id: data
                    .get("target_id")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
                expanded: data
                    .get("expanded")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                children: LazyChildren::fully_materialized(children),
            },
            ElementWidget::PrefField => ViewKind::PrefField {
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
                locked: data
                    .get("locked")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                options: match data.get("options") {
                    Some(Value::Array(arr)) => arr.clone(),
                    _ => vec![],
                },
                children: LazyChildren::fully_materialized(children),
            },
            ElementWidget::TableRow => ViewKind::TableRow { data },
        };
        Self {
            entity: Arc::new(HashMap::new()),
            kind,
            ..Default::default()
        }
    }

    /// Create a leaf node. Maps widget name to typed variant.
    pub fn leaf(widget: impl Into<String>, value: Value) -> Self {
        let widget = widget.into();
        let kind = match parse_widget::<LeafWidget>(
            "leaf",
            "text|badge|icon|checkbox|editable_text|rendered_text|dropdown|secret_text|platform_action",
            &widget,
        ) {
            LeafWidget::Text => ViewKind::Text {
                content: value.to_display_string(),
                bold: false,
                size: default_text_size(),
                color: None,
            },
            LeafWidget::Badge => ViewKind::Badge {
                label: value.to_display_string(),
            },
            LeafWidget::Icon => ViewKind::Icon {
                name: value.as_string().unwrap_or("circle").to_string(),
                size: default_icon_size(),
            },
            LeafWidget::Checkbox => ViewKind::Checkbox {
                checked: value.as_bool().unwrap_or(false),
            },
            LeafWidget::EditableText => ViewKind::EditableText {
                content: value.to_display_string(),
                field: default_editable_field(),
            },
            LeafWidget::RenderedText => ViewKind::RenderedText {
                content: value.to_display_string(),
                field: default_editable_field(),
            },
            // Snapshot tree has no rich variant for these preference-field
            // leaves; render the underlying value as plain text (the live
            // `ReactiveViewModel` path renders them properly). This preserves
            // the pre-enum `_ => Text` behavior, now made explicit.
            LeafWidget::Dropdown | LeafWidget::SecretText | LeafWidget::PlatformAction => {
                ViewKind::Text {
                    content: value.to_display_string(),
                    bold: false,
                    size: default_text_size(),
                    color: None,
                }
            }
        };
        Self {
            entity: Arc::new(HashMap::new()),
            kind,
            ..Default::default()
        }
    }

    pub fn live_block(block_id: impl Into<String>, content: ViewModel) -> Self {
        Self {
            kind: ViewKind::LiveBlock {
                block_id: block_id.into(),
                content: Box::new(content),
            },
            ..Default::default()
        }
    }

    pub fn drawer(
        block_id: impl Into<String>,
        mode: DrawerMode,
        width: f32,
        child: ViewModel,
    ) -> Self {
        Self {
            kind: ViewKind::Drawer {
                block_id: block_id.into(),
                mode,
                width,
                child: Box::new(child),
            },
            ..Default::default()
        }
    }

    pub fn with_layout_hint(mut self, hint: LayoutHint) -> Self {
        self.layout_hint = hint;
        self
    }

    pub fn error(_: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: ViewKind::Error {
                message: message.into(),
            },
            ..Default::default()
        }
    }

    pub fn empty() -> Self {
        Self {
            kind: ViewKind::Empty,
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
            ViewKind::Text { content, bold, .. } => {
                let bold_marker = if *bold { " (bold)" } else { "" };
                let _ = writeln!(out, "{pad}text {content:?}{bold_marker}{ops_suffix}");
            }
            ViewKind::Badge { label } => {
                let _ = writeln!(out, "{pad}badge {label:?}{ops_suffix}");
            }
            ViewKind::Icon { name, .. } => {
                let _ = writeln!(out, "{pad}icon {name:?}{ops_suffix}");
            }
            ViewKind::Checkbox { checked } => {
                let _ = writeln!(out, "{pad}checkbox({checked}){ops_suffix}");
            }
            ViewKind::Spacer { .. } => {
                let _ = writeln!(out, "{pad}spacer{ops_suffix}");
            }
            ViewKind::Divider => {
                let _ = writeln!(out, "{pad}divider{ops_suffix}");
            }
            ViewKind::EditableText { content, .. } => {
                let _ = writeln!(out, "{pad}editable_text {content:?}{ops_suffix}");
            }
            ViewKind::RenderedText { content, .. } => {
                let _ = writeln!(out, "{pad}rendered_text {content:?}{ops_suffix}");
            }
            ViewKind::Image { path, alt, .. } => {
                let label = if alt.is_empty() {
                    path.as_str()
                } else {
                    alt.as_str()
                };
                let _ = writeln!(out, "{pad}image({label:?}){ops_suffix}");
            }

            // Nodes with lazy children
            ViewKind::List { children, .. }
            | ViewKind::Tree { children }
            | ViewKind::Table { children }
            | ViewKind::Outline { children }
            | ViewKind::Board { children, .. }
            | ViewKind::QueryResult { children } => {
                let name = self.widget_name().unwrap_or("collection");
                let _ = writeln!(
                    out,
                    "{pad}{name} [{} items]{ops_suffix}",
                    children.items.len()
                );
                for item in &children.items {
                    item.fmt_indent(out, indent + 1);
                }
            }
            ViewKind::Row { children, .. }
            | ViewKind::Columns { children, .. }
            | ViewKind::Column { children, .. }
            | ViewKind::TreeItem { children, .. } => {
                let name = self.widget_name().unwrap_or("layout");
                let _ = writeln!(out, "{pad}{name}{ops_suffix}");
                for child in &children.items {
                    child.fmt_indent(out, indent + 1);
                }
            }
            ViewKind::Section { title, children } => {
                let _ = writeln!(out, "{pad}section({title:?}){ops_suffix}");
                for child in &children.items {
                    child.fmt_indent(out, indent + 1);
                }
            }

            // Elements
            ViewKind::SourceBlock {
                language, content, ..
            } => {
                let _ = writeln!(
                    out,
                    "{pad}source_block({language}) {}{ops_suffix}",
                    &content[..content.len().min(40)]
                );
            }
            ViewKind::SourceEditor {
                language, content, ..
            } => {
                let _ = writeln!(
                    out,
                    "{pad}source_editor({language}) {}{ops_suffix}",
                    &content[..content.len().min(40)]
                );
            }
            ViewKind::BlockOperations { operations } => {
                let _ = writeln!(out, "{pad}block_operations({operations}){ops_suffix}");
            }
            ViewKind::StateToggle {
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
            ViewKind::ExpandToggle {
                target_id,
                expanded,
                children,
            } => {
                let icon = if *expanded { "\u{25BC}" } else { "\u{25B6}" };
                let _ = writeln!(out, "{pad}expand_toggle({icon} {target_id}){ops_suffix}");
                for child in &children.items {
                    child.fmt_indent(out, indent + 1);
                }
            }
            ViewKind::PrefField {
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
            ViewKind::TableRow { data } => {
                let fields = format_data_inline(data);
                let _ = writeln!(out, "{pad}table_row {{{fields}}}{ops_suffix}");
            }

            // Wrappers
            ViewKind::Focusable { child } => {
                let _ = writeln!(out, "{pad}focusable{ops_suffix}");
                child.fmt_indent(out, indent + 1);
            }
            ViewKind::Selectable { child } => {
                let _ = writeln!(out, "{pad}selectable{ops_suffix}");
                child.fmt_indent(out, indent + 1);
            }
            ViewKind::Draggable { child } => {
                let _ = writeln!(out, "{pad}draggable{ops_suffix}");
                child.fmt_indent(out, indent + 1);
            }
            ViewKind::PieMenu { child, .. } => {
                let _ = writeln!(out, "{pad}pie_menu{ops_suffix}");
                child.fmt_indent(out, indent + 1);
            }
            ViewKind::DropZone { .. } => {
                let _ = writeln!(out, "{pad}drop_zone{ops_suffix}");
            }
            ViewKind::ViewModeSwitcher { child, .. } => {
                let _ = writeln!(out, "{pad}view_mode_switcher{ops_suffix}");
                child.fmt_indent(out, indent + 1);
            }
            ViewKind::Drawer {
                block_id,
                mode,
                width,
                child,
            } => {
                let mode_str = match mode {
                    DrawerMode::Shrink => "shrink",
                    DrawerMode::Overlay => "overlay",
                };
                let _ = writeln!(
                    out,
                    "{pad}drawer({block_id}, {mode_str}, {width}px){ops_suffix}"
                );
                child.fmt_indent(out, indent + 1);
            }
            ViewKind::Card { accent, children } => {
                let _ = writeln!(out, "{pad}card({accent}){ops_suffix}");
                for child in &children.items {
                    child.fmt_indent(out, indent + 1);
                }
            }
            ViewKind::ChatBubble {
                sender, children, ..
            } => {
                let _ = writeln!(out, "{pad}chat_bubble({sender}){ops_suffix}");
                for child in &children.items {
                    child.fmt_indent(out, indent + 1);
                }
            }
            ViewKind::Collapsible {
                header, children, ..
            } => {
                let _ = writeln!(out, "{pad}collapsible({header}){ops_suffix}");
                for child in &children.items {
                    child.fmt_indent(out, indent + 1);
                }
            }
            ViewKind::OnHover { children } => {
                let _ = writeln!(out, "{pad}on_hover{ops_suffix}");
                for child in &children.items {
                    child.fmt_indent(out, indent + 1);
                }
            }
            ViewKind::BottomDock { children } => {
                let _ = writeln!(out, "{pad}bottom_dock{ops_suffix}");
                for child in &children.items {
                    child.fmt_indent(out, indent + 1);
                }
            }
            ViewKind::OpButton {
                op_name,
                target_id,
                display_name,
            } => {
                let _ = writeln!(
                    out,
                    "{pad}op_button({op_name}, target={target_id}, \"{display_name}\"){ops_suffix}"
                );
            }

            // Special
            ViewKind::LiveBlock { block_id, content } => {
                let _ = writeln!(out, "{pad}live_block({block_id}){ops_suffix}");
                content.fmt_indent(out, indent + 1);
            }
            ViewKind::LiveQuery { content, .. } => {
                let _ = writeln!(out, "{pad}live_query{ops_suffix}");
                content.fmt_indent(out, indent + 1);
            }
            ViewKind::RenderBlock { content } => {
                let _ = writeln!(out, "{pad}render_entity{ops_suffix}");
                content.fmt_indent(out, indent + 1);
            }
            ViewKind::Error { message } => {
                let _ = writeln!(out, "{pad}ERROR: {message}");
            }
            ViewKind::Empty => {
                let _ = writeln!(out, "{pad}(empty)");
            }
            ViewKind::Loading => {
                let _ = writeln!(out, "{pad}(loading)");
            }
        }
    }

    /// Collect drawer `(block_id, mode)` pairs from the tree, in depth-first
    /// order. The mode lets callers (e.g. the toolbar drawer toggles) compute a
    /// drawer's mode-aware default open state.
    pub fn collect_drawers(&self) -> Vec<(String, DrawerMode)> {
        let mut drawers = Vec::new();
        self.collect_drawers_recursive(&mut drawers);
        drawers
    }

    fn collect_drawers_recursive(&self, drawers: &mut Vec<(String, DrawerMode)>) {
        if let ViewKind::Drawer { block_id, mode, .. } = &self.kind {
            drawers.push((block_id.clone(), *mode));
        }
        for child in self.children() {
            child.collect_drawers_recursive(drawers);
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
            ViewKind::LiveBlock { block_id, content } => {
                ids.push(block_id.clone());
                content.collect_ids_recursive(ids);
            }
            ViewKind::TableRow { data } => {
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
            ViewKind::Row { children, .. }
            | ViewKind::Section { children, .. }
            | ViewKind::List { children, .. }
            | ViewKind::Tree { children }
            | ViewKind::Outline { children }
            | ViewKind::Table { children }
            | ViewKind::Columns { children, .. }
            | ViewKind::Board { children, .. }
            | ViewKind::Column { children, .. }
            | ViewKind::QueryResult { children }
            | ViewKind::PrefField { children, .. }
            | ViewKind::TreeItem { children, .. }
            | ViewKind::Card { children, .. }
            | ViewKind::ChatBubble { children, .. }
            | ViewKind::Collapsible { children, .. }
            | ViewKind::OnHover { children, .. }
            | ViewKind::ExpandToggle { children, .. }
            | ViewKind::BottomDock { children, .. } => &children.items,

            // Box<ViewModel> wrappers
            ViewKind::Focusable { child }
            | ViewKind::Selectable { child }
            | ViewKind::Draggable { child }
            | ViewKind::Drawer { child, .. }
            | ViewKind::PieMenu { child, .. }
            | ViewKind::ViewModeSwitcher { child, .. }
            | ViewKind::LiveBlock { content: child, .. }
            | ViewKind::LiveQuery { content: child, .. }
            | ViewKind::RenderBlock { content: child } => std::slice::from_ref(child.as_ref()),

            // Leaf nodes
            ViewKind::Text { .. }
            | ViewKind::Badge { .. }
            | ViewKind::Icon { .. }
            | ViewKind::Checkbox { .. }
            | ViewKind::Divider
            | ViewKind::Spacer { .. }
            | ViewKind::EditableText { .. }
            | ViewKind::RenderedText { .. }
            | ViewKind::Image { .. }
            | ViewKind::SourceBlock { .. }
            | ViewKind::SourceEditor { .. }
            | ViewKind::BlockOperations { .. }
            | ViewKind::StateToggle { .. }
            | ViewKind::TableRow { .. }
            | ViewKind::OpButton { .. }
            | ViewKind::DropZone { .. }
            | ViewKind::Error { .. }
            | ViewKind::Empty
            | ViewKind::Loading => &[],
        }
    }

    /// Collect entity IDs of all blocks that have a StateToggle in their
    /// subtree.
    pub fn state_toggle_block_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        self.collect_state_toggle_ids(&mut ids);
        ids
    }

    fn collect_state_toggle_ids(&self, ids: &mut Vec<String>) {
        if matches!(self.kind, ViewKind::StateToggle { .. }) {
            if let Some(Value::String(id)) = self.entity.get("id") {
                ids.push(id.clone());
            }
        }
        for child in self.children() {
            child.collect_state_toggle_ids(ids);
        }
    }

    /// The widget name (e.g. "list", "row", "text").
    pub fn widget_name(&self) -> Option<&str> {
        Some(match &self.kind {
            ViewKind::Text { .. } => "text",
            ViewKind::Badge { .. } => "badge",
            ViewKind::Icon { .. } => "icon",
            ViewKind::Checkbox { .. } => "checkbox",
            ViewKind::Divider => "divider",
            ViewKind::Spacer { .. } => "spacer",
            ViewKind::EditableText { .. } => "editable_text",
            ViewKind::RenderedText { .. } => "rendered_text",
            ViewKind::Image { .. } => "image",
            ViewKind::Row { .. } => "row",
            ViewKind::Section { .. } => "section",
            ViewKind::List { .. } => "list",
            ViewKind::Tree { .. } => "tree",
            ViewKind::Outline { .. } => "outline",
            ViewKind::Table { .. } => "table",
            ViewKind::Columns { .. } => "columns",
            ViewKind::Board { .. } => "board",
            ViewKind::Column { .. } => "column",
            ViewKind::SourceBlock { .. } => "source_block",
            ViewKind::SourceEditor { .. } => "source_editor",
            ViewKind::BlockOperations { .. } => "block_operations",
            ViewKind::StateToggle { .. } => "state_toggle",
            ViewKind::ExpandToggle { .. } => "expand_toggle",
            ViewKind::PrefField { .. } => "pref_field",
            ViewKind::QueryResult { .. } => "query_result",
            ViewKind::TableRow { .. } => "table_row",
            ViewKind::Focusable { .. } => "focusable",
            ViewKind::Selectable { .. } => "selectable",
            ViewKind::Draggable { .. } => "draggable",
            ViewKind::PieMenu { .. } => "pie_menu",
            ViewKind::DropZone { .. } => "drop_zone",
            ViewKind::ViewModeSwitcher { .. } => "view_mode_switcher",
            ViewKind::Drawer { .. } => "drawer",
            ViewKind::Card { .. } => "card",
            ViewKind::ChatBubble { .. } => "chat_bubble",
            ViewKind::Collapsible { .. } => "collapsible",
            ViewKind::OnHover { .. } => "on_hover",
            ViewKind::BottomDock { .. } => "bottom_dock",
            ViewKind::OpButton { .. } => "op_button",
            ViewKind::LiveBlock { .. } => "live_block",
            ViewKind::LiveQuery { .. } => "live_query",
            ViewKind::RenderBlock { .. } => "render_entity",
            ViewKind::Error { .. } => "error",
            ViewKind::TreeItem { .. } => "tree_item",
            ViewKind::Empty | ViewKind::Loading => return None,
        })
    }

    /// Extract entity ID from element data or LiveBlock block_id.
    pub fn entity_id(&self) -> Option<EntityUri> {
        match &self.kind {
            ViewKind::TableRow { data } => data
                .get("id")
                .and_then(|v| v.as_string())
                .map(holon_api::entity_uri_from_id_str),
            ViewKind::LiveBlock { block_id, .. } => Some(
                EntityUri::parse(block_id)
                    .expect("live_block block_id must be a schemed EntityUri"),
            ),
            _ => self
                .entity
                .get("id")
                .and_then(|v| v.as_string())
                .map(holon_api::entity_uri_from_id_str),
        }
    }

    /// Extract the entity name from this node's ID scheme (e.g. `"block:uuid"`
    /// → `"block"`), falling back to an explicit `entity_name` field.
    pub fn entity_name(&self) -> Option<EntityName> {
        if let Some(Value::String(id)) = self.entity.get("id") {
            if let Some(scheme) = id.split_once(':').map(|(s, _)| s) {
                return Some(EntityName::Named(scheme.to_string()));
            }
        }
        if let Some(Value::String(s)) = self.entity.get("entity_name") {
            return Some(EntityName::Named(s.to_string()));
        }
        None
    }

    /// Extract the row ID from this node's entity data.
    pub fn row_id(&self) -> Option<String> {
        match self.entity.get("id") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Integer(i)) => Some(i.to_string()),
            _ => None,
        }
    }

    /// Find the first `EditableText` descendant whose entity `id` matches
    /// `entity_id`.
    pub fn find_editable_text(&self, entity_id: &str) -> Option<&ViewModel> {
        if matches!(&self.kind, ViewKind::EditableText { .. }) {
            if self
                .entity
                .get("id")
                .and_then(|v| v.as_string())
                .map_or(false, |id| id == entity_id)
            {
                return Some(self);
            }
        }
        self.children()
            .iter()
            .find_map(|c| c.find_editable_text(entity_id))
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
    fn drawer_mode_default_open() {
        // Desktop sidebars (shrink) start open; phone overlays start closed so
        // they don't cover the main panel on first paint.
        assert!(DrawerMode::Shrink.default_open());
        assert!(!DrawerMode::Overlay.default_open());
    }

    #[test]
    fn collect_drawers_reports_id_and_mode() {
        let tree = ViewModel::layout(
            "columns",
            vec![
                ViewModel::drawer("left", DrawerMode::Overlay, 260.0, ViewModel::empty()),
                ViewModel::live_block("main", ViewModel::empty()),
                ViewModel::drawer("right", DrawerMode::Shrink, 260.0, ViewModel::empty()),
            ],
        );

        let drawers = tree.collect_drawers();
        assert_eq!(
            drawers,
            vec![
                ("left".to_string(), DrawerMode::Overlay),
                ("right".to_string(), DrawerMode::Shrink),
            ]
        );
    }

    #[test]
    fn pretty_print_nested() {
        let tree = ViewModel::layout(
            "columns",
            vec![ViewModel::collection(
                "list",
                vec![
                    ViewModel::live_block(
                        "a",
                        ViewModel::element(
                            "table_row",
                            Arc::new(HashMap::from([
                                ("id".into(), Value::String("a".into())),
                                ("content".into(), Value::String("First".into())),
                            ])),
                            vec![],
                        ),
                    ),
                    ViewModel::live_block(
                        "b",
                        ViewModel::element(
                            "table_row",
                            Arc::new(HashMap::from([
                                ("id".into(), Value::String("b".into())),
                                ("content".into(), Value::String("Second".into())),
                            ])),
                            vec![],
                        ),
                    ),
                ],
            )],
        );

        let output = tree.pretty_print(0);
        assert!(output.contains("columns"));
        assert!(output.contains("list [2 items]"));
        assert!(output.contains("live_block(a)"));
        assert!(output.contains("live_block(b)"));
    }

    #[test]
    fn collect_entity_ids_mixed() {
        let tree = ViewModel::layout(
            "column",
            vec![
                ViewModel::live_block(
                    "ref-1",
                    ViewModel::element(
                        "table_row",
                        Arc::new(HashMap::from([(
                            "id".into(),
                            Value::String("inner-1".into()),
                        )])),
                        vec![],
                    ),
                ),
                ViewModel::element(
                    "table_row",
                    Arc::new(HashMap::from([(
                        "id".into(),
                        Value::String("row-1".into()),
                    )])),
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
            Arc::new(HashMap::from([("id".into(), Value::String("abc".into()))])),
            vec![],
        );
        assert_eq!(elem.entity_id(), Some(EntityUri::block("abc")));

        let bref = ViewModel::live_block("block:xyz", ViewModel::empty());
        assert_eq!(bref.entity_id(), Some(EntityUri::block("xyz")));

        assert_eq!(ViewModel::empty().entity_id(), None);
    }

    #[test]
    fn lazy_children_fully_materialized() {
        let lc = LazyChildren::fully_materialized(vec![ViewModel::empty(), ViewModel::empty()]);
        assert_eq!(lc.items.len(), 2);
    }
}
