//! Persistent reactive ViewModel — the primary representation for all frontends.
//!
//! Each node owns reactive inputs (`Mutable<RenderExpr>`, `Mutable<Arc<DataRow>>`)
//! and self-interprets when any input changes. Changes push DOWN the tree —
//! no external tree walks, no reconciliation.
//!
//! ```text
//! (expr, data) → interpret → display → frontend subscribes
//!                  ↓
//!            children receive push-down updates
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::{Mutable, ReadOnlyMutable};
use holon_api::render_types::{OperationWiring, RenderExpr};
use holon_api::widget_spec::DataRow;
use holon_api::{EntityName, EntityUri, Value};

use crate::input_trigger::InputTrigger;
use crate::render_context::LayoutHint;
use crate::view_model::{DrawerMode, LazyChildren, ViewKind, ViewModel};

/// Self-interpretation function stored on each node.
///
/// Takes `(expr, data)` and returns the recomputed props. Captured by the
/// collection driver at node-creation time — it closes over `BuilderServices`
/// and the current container-query `Mutable<Option<AvailableSpace>>`, so the
/// node can recompute its own props when data (or later, expr) changes without
/// the driver having to call `services.interpret()`.
pub type InterpretFn =
    Arc<dyn Fn(&RenderExpr, &Arc<DataRow>) -> HashMap<String, Value> + Send + Sync>;

// ── CollectionData (builder-time helper) ───────────────────────────────

/// Builder-time helper for the `widget_builder!` macro's Collection extraction.
/// NOT part of the persistent node structure — just plumbing between the macro's
/// extraction code and the manual builder body.
pub enum CollectionData {
    Streaming {
        item_template: RenderExpr,
        data_source: std::sync::Arc<dyn holon_api::ReactiveRowProvider>,
        sort_key: Option<String>,
    },
    Static {
        items: Vec<ReactiveViewModel>,
    },
}

impl CollectionData {
    pub fn into_static_items(self) -> Vec<ReactiveViewModel> {
        match self {
            Self::Static { items } => items,
            Self::Streaming { .. } => panic!(
                "CollectionData::into_static_items called on a Streaming variant — \
                 this builder does not support streaming collections."
            ),
        }
    }
}

// ── CollectionVariant ──────────────────────────────────────────────────

/// Layout descriptor for a collection-shaped widget.
///
/// Carries the registered `LayoutSpec` (driving the streaming runtime's
/// flat/hierarchical decision and surfacing the layout's name) plus the
/// resolved `gap` value from the call site. Replaces the previous closed
/// `enum` so new layouts can register without touching shared infra; see
/// `crate::collection_layout` for the registry.
#[derive(Clone, Debug, PartialEq)]
pub struct CollectionVariant {
    pub spec: crate::collection_layout::LayoutSpec,
    pub gap: f32,
}

impl CollectionVariant {
    pub fn new(spec: crate::collection_layout::LayoutSpec, gap: f32) -> Self {
        Self { spec, gap }
    }

    /// Look up a registered layout by name and pair it with a gap. Panics
    /// in tests / fixtures when the name isn't registered — production
    /// callers that consume user-supplied names should use
    /// `from_name_optional` instead.
    pub fn from_name(name: &str, gap: f32) -> Option<Self> {
        crate::collection_layout::lookup_layout(name).map(|spec| Self { spec, gap })
    }

    /// Builtin convenience constructors. Each one panics if the named
    /// layout isn't registered — only callable for layouts the registry
    /// is guaranteed to contain (e.g. `list`, `tree`, …) at startup.
    fn builtin(name: &'static str, gap: f32) -> Self {
        Self::from_name(name, gap)
            .unwrap_or_else(|| panic!("`{name}` layout is registered as a builtin"))
    }

    pub fn tree() -> Self {
        Self::builtin("tree", 0.0)
    }
    pub fn outline() -> Self {
        Self::builtin("outline", 0.0)
    }
    pub fn table() -> Self {
        Self::builtin("table", 0.0)
    }
    pub fn list(gap: f32) -> Self {
        Self::builtin("list", gap)
    }
    pub fn columns(gap: f32) -> Self {
        Self::builtin("columns", gap)
    }

    pub fn name(&self) -> &str {
        &self.spec.name
    }

    pub fn shape(&self) -> crate::collection_layout::LayoutShape {
        self.spec.shape
    }

    pub fn is_hierarchical(&self) -> bool {
        matches!(
            self.spec.shape,
            crate::collection_layout::LayoutShape::Hierarchical
        )
    }
}

/// Determine the `CollectionVariant` from a render expression's function name.
///
/// Returns `None` for non-collection expressions (non-FunctionCall or
/// unrecognized function names).
pub fn collection_variant_of(expr: &RenderExpr) -> Option<CollectionVariant> {
    let (name, args) = match expr {
        RenderExpr::FunctionCall { name, args } => (name.as_str(), args),
        _ => return None,
    };

    let spec = crate::collection_layout::lookup_layout(name)?;

    // Extract `gap:` named arg if the call site overrides it; fall back to
    // the layout's declared default. Layouts that don't care about gap
    // (tree, table, …) just see 0.0.
    let gap = args
        .iter()
        .find(|a| a.name.as_deref() == Some("gap"))
        .and_then(|a| match &a.value {
            RenderExpr::Literal {
                value: Value::Float(f),
            } => Some(*f as f32),
            RenderExpr::Literal {
                value: Value::Integer(i),
            } => Some(*i as f32),
            _ => None,
        })
        .unwrap_or(spec.default_gap);

    Some(CollectionVariant { spec, gap })
}

/// Returns true if both variants are the same kind (same registered name).
/// Used by `view_mode_switcher`'s fast-path to detect intra-variant switches
/// (e.g. board → board with a different `item_template`) where the existing
/// `ReactiveView` can be re-used vs. a full rebuild.
pub fn variants_match(a: Option<CollectionVariant>, b: Option<CollectionVariant>) -> bool {
    match (a, b) {
        (Some(av), Some(bv)) => av.spec.name == bv.spec.name,
        _ => false,
    }
}

/// Extract the `item_template` (or `item`) named arg from a collection expression.
pub fn extract_item_template(collection_expr: &RenderExpr) -> Option<RenderExpr> {
    match collection_expr {
        RenderExpr::FunctionCall { args, .. } => args
            .iter()
            .find(|a| {
                a.name.as_deref() == Some("item_template") || a.name.as_deref() == Some("item")
            })
            .map(|a| a.value.clone()),
        _ => None,
    }
}

// ── ReactiveSlot ───────────────────────────────────────────────────────

/// Reactive content slot — content changes over time.
///
/// Used by `LiveBlock`, `LiveQuery`, and `ViewModeSwitcher` to hold content
/// that is populated asynchronously. The frontend subscribes to the `Mutable`
/// and re-renders when content changes.
pub struct ReactiveSlot {
    pub content: Mutable<Arc<ReactiveViewModel>>,
}

impl ReactiveSlot {
    pub fn new(content: ReactiveViewModel) -> Self {
        Self {
            content: Mutable::new(Arc::new(content)),
        }
    }

    pub fn empty() -> Self {
        Self::new(ReactiveViewModel::empty())
    }

    pub fn snapshot(&self) -> ViewModel {
        self.content.lock_ref().snapshot()
    }

    pub fn snapshot_resolved(&self, resolve_block: &dyn Fn(&EntityUri) -> ViewModel) -> ViewModel {
        self.content.lock_ref().snapshot_resolved(resolve_block)
    }
}

// ── ReactiveViewModel ──────────────────────────────────────────────────

/// A persistent reactive ViewModel node.
///
/// Each node owns its render expression and data row as reactive Mutables.
/// When either changes, the node self-interprets and pushes updates to children.
///
/// This replaces the old snapshot-based `ReactiveViewModel` + `ReactiveViewKind`
/// enum. Widget type is determined by the `expr` function name, not an enum tag.
pub struct ReactiveViewModel {
    /// The render expression this node was built from.
    /// For leaf nodes: `text(...)`, `badge(...)`, etc.
    /// For containers: `row(...)`, `column(...)`, etc.
    pub expr: Mutable<RenderExpr>,

    /// The data row this node is interpreting.
    ///
    /// `ReadOnlyMutable` — by design. The only writable handle to a row's
    /// cell lives inside `ReactiveRowSet.data` (private to that struct);
    /// `apply_change` is the sole writer. Every node downstream — including
    /// every leaf widget — holds a `ReadOnlyMutable` clone, so attempts to
    /// `.set()` row data from a leaf are a **compile error**. This is the
    /// type-system enforcement of the one-writer rule. UI-local state
    /// (expand, focus, view mode, scroll) lives in separate fields and
    /// stays freely mutable.
    pub data: ReadOnlyMutable<Arc<DataRow>>,

    /// Static children (layout containers, expand toggle header).
    pub children: Vec<Arc<ReactiveViewModel>>,

    /// Reactive collection backing this node's children, if any.
    /// When present, the frontend subscribes to the view's MutableVec.
    pub collection: Option<Arc<crate::reactive_view::ReactiveView>>,

    /// Deferred content slot (live_block, live_query, view_mode_switcher).
    pub slot: Option<ReactiveSlot>,

    /// Expand/collapse state — shared handle from the engine's cache.
    pub expanded: Option<Mutable<bool>>,

    /// Operations available at this node.
    pub operations: Vec<OperationWiring>,

    /// Input triggers.
    pub triggers: Vec<InputTrigger>,

    /// Layout hint.
    pub layout_hint: LayoutHint,

    /// Additional typed properties extracted during interpretation.
    /// Reactive so data-only updates can refresh props without replacing
    /// the Arc<ReactiveViewModel> in the MutableVec.
    pub props: Mutable<HashMap<String, Value>>,

    /// Captured render context for deferred re-interpretation
    /// (expand_toggle content, view_mode_switcher mode switching).
    pub render_ctx: Option<crate::render_context::RenderContext>,

    /// Self-interpretation closure. When set, `set_data()` recomputes `props`
    /// automatically — the collection driver only needs to push new data,
    /// not re-run the full interpret pipeline.
    pub interpret_fn: Option<InterpretFn>,

    /// Tasks owned by this node (e.g. signal subscriptions that update
    /// `props` on `data` changes). Aborted when the node is dropped so
    /// removed rows don't leak background work.
    pub subscriptions: Vec<DropTask>,
}

/// A `tokio::task::JoinHandle<()>` that aborts the task on drop.
///
/// Used by leaf builders that spawn signal subscriptions (e.g.
/// `state_toggle` watching its row's `data` Mutable). Storing the handle
/// inside the `ReactiveViewModel` ties task lifetime to node lifetime —
/// when the collection driver removes the row, the VM is dropped, the
/// `DropTask` field is dropped, and the task is aborted.
pub struct DropTask(tokio::task::AbortHandle);

impl DropTask {
    pub fn new(handle: tokio::task::JoinHandle<()>) -> Self {
        Self(handle.abort_handle())
    }
}

impl Drop for DropTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl std::fmt::Debug for DropTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DropTask")
    }
}

// ── Accessors ──────────────────────────────────────────────────────────

impl ReactiveViewModel {
    /// Widget name derived from the expression's function name.
    pub fn widget_name(&self) -> Option<String> {
        match &*self.expr.lock_ref() {
            RenderExpr::FunctionCall { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// The data row this node represents.
    pub fn entity(&self) -> Arc<DataRow> {
        self.data.get_cloned()
    }

    /// Update this node's *structural* mutables (expr, props) in place from
    /// a freshly-interpreted node.
    ///
    /// `data` is **not** patched — it's a `ReadOnlyMutable` cloned from the
    /// shared per-row cell in `ReactiveRowSet`. Row-level updates flow
    /// through the per-row signal automatically; nothing needs to push them
    /// down. Only structural fields (which can change without a row update,
    /// e.g. variant switching) are copied here.
    ///
    /// NOTE: Does NOT copy `interpret_fn` from `fresh` — the existing node's
    /// self-interpretation closure is preserved (it captures the right services
    /// and space handles for THIS node's position in the tree).
    pub fn patch_mutables(&self, fresh: &ReactiveViewModel) {
        self.expr.set(fresh.expr.get_cloned());
        self.props.set(fresh.props.get_cloned());
    }

    /// Set the render expression and recompute props.
    ///
    /// Used when a shared template `Mutable<RenderExpr>` changes — each node
    /// that shares the template recomputes its own props from its own data.
    pub fn set_expr(&self, new_expr: RenderExpr) {
        self.expr.set(new_expr.clone());
        if let Some(ref f) = self.interpret_fn {
            self.props.set(f(&new_expr, &self.data.get_cloned()));
        }
    }

    /// Receive a structural update and apply it in place.
    ///
    /// The node handles its own update: refreshes data/expr/props, then
    /// pushes changes down to children. Matching children (same widget name
    /// at the same position) are updated in place — their Mutable handles,
    /// expand states, and GPUI entity caches survive. Non-matching children
    /// are adopted from the fresh tree.
    ///
    /// Use this when you own the root node (e.g. `ReactiveShell::current_tree`).
    /// For the `Arc<Self>` case use `with_update`.
    pub fn apply_update(&mut self, fresh: &ReactiveViewModel) {
        self.patch_mutables(fresh);
        self.children = Self::push_down_children(&self.children, &fresh.children);
        self.collection = fresh.collection.clone();
        self.slot = Self::push_down_slot(&self.slot, &fresh.slot);
        self.operations = fresh.operations.clone();
        self.triggers = fresh.triggers.clone();
        self.layout_hint = fresh.layout_hint;
        self.render_ctx = fresh.render_ctx.clone();
    }

    /// Produce an updated copy of this node, preserving Mutable handles.
    ///
    /// Same semantics as `apply_update` but returns a new `ReactiveViewModel`
    /// instead of modifying `self`. Use when the node is behind an `Arc`.
    pub fn with_update(&self, fresh: &ReactiveViewModel) -> ReactiveViewModel {
        self.patch_mutables(fresh);
        ReactiveViewModel {
            expr: Mutable::new(self.expr.get_cloned()),
            // Share the existing per-row signal cell — `data` is a
            // `ReadOnlyMutable` clone of the cell owned by `ReactiveRowSet`.
            // Cloning preserves the shared `Arc<MutableState>` so the new
            // node sees CDC updates through the same signal.
            data: self.data.clone(),
            children: Self::push_down_children(&self.children, &fresh.children),
            collection: fresh.collection.clone(),
            slot: Self::push_down_slot(&self.slot, &fresh.slot),
            expanded: self.expanded.clone(),
            operations: fresh.operations.clone(),
            triggers: fresh.triggers.clone(),
            layout_hint: fresh.layout_hint,
            props: Mutable::new(self.props.get_cloned()),
            render_ctx: fresh.render_ctx.clone(),
            interpret_fn: self.interpret_fn.clone(),
            subscriptions: Vec::new(),
        }
    }

    /// Push updates down to children, preserving matching nodes.
    ///
    /// At each position: same widget name → update in place + recurse;
    /// different widget name → adopt fresh. Extra fresh → adopt. Extra old → drop.
    fn push_down_children(
        old: &[Arc<ReactiveViewModel>],
        fresh: &[Arc<ReactiveViewModel>],
    ) -> Vec<Arc<ReactiveViewModel>> {
        let mut result = Vec::with_capacity(fresh.len());

        for (i, fresh_child) in fresh.iter().enumerate() {
            if let Some(old_child) = old.get(i) {
                if old_child.widget_name() == fresh_child.widget_name() {
                    old_child.patch_mutables(fresh_child);

                    let pushed =
                        Self::push_down_children(&old_child.children, &fresh_child.children);
                    if pushed.len() != old_child.children.len()
                        || pushed
                            .iter()
                            .zip(old_child.children.iter())
                            .any(|(a, b)| !Arc::ptr_eq(a, b))
                    {
                        result.push(Arc::new(ReactiveViewModel {
                            expr: Mutable::new(old_child.expr.get_cloned()),
                            // Share the existing per-row signal cell — see
                            // `with_update` for rationale.
                            data: old_child.data.clone(),
                            children: pushed,
                            collection: fresh_child.collection.clone(),
                            slot: Self::push_down_slot(&old_child.slot, &fresh_child.slot),
                            expanded: old_child.expanded.clone(),
                            operations: fresh_child.operations.clone(),
                            triggers: fresh_child.triggers.clone(),
                            layout_hint: fresh_child.layout_hint,
                            props: Mutable::new(old_child.props.get_cloned()),
                            render_ctx: fresh_child.render_ctx.clone(),
                            interpret_fn: old_child.interpret_fn.clone(),
                            subscriptions: Vec::new(),
                            ..ReactiveViewModel::empty()
                        }));
                    } else {
                        // Children unchanged structurally — keep original Arc.
                        // Still update slot and collection through interior mutability
                        // so VMS, live_block, and other slot-bearing nodes see fresh content.
                        if let (Some(old_slot), Some(fresh_slot)) =
                            (&old_child.slot, &fresh_child.slot)
                        {
                            old_slot.content.set(fresh_slot.content.get_cloned());
                        }
                        result.push(old_child.clone());
                    }
                } else {
                    result.push(fresh_child.clone());
                }
            } else {
                result.push(fresh_child.clone());
            }
        }

        result
    }

    fn push_down_slot(
        old: &Option<ReactiveSlot>,
        fresh: &Option<ReactiveSlot>,
    ) -> Option<ReactiveSlot> {
        match (old, fresh) {
            (Some(old_slot), Some(fresh_slot)) => {
                old_slot.content.set(fresh_slot.content.get_cloned());
                Some(ReactiveSlot {
                    content: old_slot.content.clone(),
                })
            }
            (_, Some(fresh_slot)) => Some(ReactiveSlot {
                content: fresh_slot.content.clone(),
            }),
            (_, None) => None,
        }
    }

    /// Extract entity name from the data row's ID scheme.
    pub fn entity_name(&self) -> Option<EntityName> {
        let data = self.data.get_cloned();
        if let Some(Value::String(id)) = data.get("id") {
            if let Some(scheme) = id.split_once(':').map(|(s, _)| s) {
                return Some(EntityName::Named(scheme.to_string()));
            }
        }
        if let Some(Value::String(s)) = data.get("entity_name") {
            return Some(EntityName::Named(s.to_string()));
        }
        None
    }

    /// Extract the row ID from the data row.
    pub fn row_id(&self) -> Option<String> {
        let data = self.data.get_cloned();
        match data.get("id") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Integer(i)) => Some(i.to_string()),
            _ => None,
        }
    }

    /// Entity ID — for live_block nodes, uses the block_id from props.
    pub fn entity_id(&self) -> Option<String> {
        let props = self.props.lock_ref();
        if let Some(Value::String(block_id)) = props.get("block_id") {
            return Some(block_id.clone());
        }
        drop(props);
        self.row_id()
    }

    /// Get a string property (owned — reads through Mutable lock).
    pub fn prop_str(&self, key: &str) -> Option<String> {
        self.props
            .lock_ref()
            .get(key)
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
    }

    /// Get a bool property.
    pub fn prop_bool(&self, key: &str) -> Option<bool> {
        self.props.lock_ref().get(key).and_then(|v| v.as_bool())
    }

    /// Get an f64 property.
    pub fn prop_f64(&self, key: &str) -> Option<f64> {
        self.props.lock_ref().get(key).and_then(|v| match v {
            Value::Float(f) => Some(*f),
            Value::Integer(i) => Some(*i as f64),
            _ => None,
        })
    }

    /// Get a Value property (cloned).
    pub fn prop_value(&self, key: &str) -> Option<Value> {
        self.props.lock_ref().get(key).cloned()
    }

    /// The intent that would be dispatched if this node were clicked.
    ///
    /// Walks `operations` for a `Trigger::Click` entry and returns an
    /// `OperationIntent` built from the descriptor's entity/op name and
    /// `bound_params`. Returns `None` for nodes without a click action.
    ///
    /// Pure read — no services, no dispatch. The caller (GPUI click handler,
    /// test driver, headless click simulator) decides what to do with the
    /// returned intent. This separation lets unit tests assert the click
    /// binding is correct without spinning up a dispatch pipeline.
    pub fn click_intent(&self) -> Option<crate::operations::OperationIntent> {
        let op = self
            .operations
            .iter()
            .find(|ow| ow.descriptor.is_click_triggered())?;
        Some(crate::operations::OperationIntent::new(
            op.descriptor.entity_name.clone(),
            op.descriptor.name.clone(),
            op.descriptor.bound_params.clone(),
        ))
    }
}

// ── Snapshot ───────────────────────────────────────────────────────────

impl ReactiveViewModel {
    /// Materialize into a static `ViewModel` by reading all current signal values.
    pub fn snapshot(&self) -> ViewModel {
        let expr = self.expr.get_cloned();
        let data = self.data.get_cloned();
        let kind = self.to_view_kind(&expr, &data, None);
        ViewModel {
            entity: data,
            kind,
            operations: self.operations.clone(),
            triggers: self.triggers.clone(),
            layout_hint: self.layout_hint,
        }
    }

    /// Materialize into a static `ViewModel`, resolving `LiveBlock` placeholders.
    pub fn snapshot_resolved(&self, resolve_block: &dyn Fn(&EntityUri) -> ViewModel) -> ViewModel {
        let expr = self.expr.get_cloned();
        let data = self.data.get_cloned();
        let kind = self.to_view_kind(&expr, &data, Some(resolve_block));
        ViewModel {
            entity: data,
            kind,
            operations: self.operations.clone(),
            triggers: self.triggers.clone(),
            layout_hint: self.layout_hint,
        }
    }

    fn to_view_kind(
        &self,
        expr: &RenderExpr,
        _data: &DataRow,
        resolve_block: Option<&dyn Fn(&EntityUri) -> ViewModel>,
    ) -> ViewKind {
        let snap = |rvm: &ReactiveViewModel| -> ViewModel {
            match resolve_block {
                Some(rb) => rvm.snapshot_resolved(rb),
                None => rvm.snapshot(),
            }
        };
        let snap_children = || -> LazyChildren {
            let items: Vec<ViewModel> = self.children.iter().map(|c| snap(c)).collect();
            LazyChildren::fully_materialized(items)
        };

        let name = match expr {
            RenderExpr::FunctionCall { name, .. } => name.as_str(),
            _ => return ViewKind::Empty,
        };

        match name {
            // Leaf nodes
            "text" => ViewKind::Text {
                content: self.prop_str("content").unwrap_or_default(),
                bold: self.prop_bool("bold").unwrap_or(false),
                size: self.prop_f64("size").unwrap_or(14.0) as f32,
                color: self.prop_str("color"),
            },
            "badge" => ViewKind::Badge {
                label: self.prop_str("label").unwrap_or_default(),
            },
            "icon" => ViewKind::Icon {
                name: self
                    .prop_str("name")
                    .unwrap_or_else(|| "circle".to_string()),
                size: self.prop_f64("size").unwrap_or(16.0) as f32,
            },
            "checkbox" => ViewKind::Checkbox {
                checked: self.prop_bool("checked").unwrap_or(false),
            },
            "spacer" => ViewKind::Spacer {
                width: self.prop_f64("width").unwrap_or(0.0) as f32,
                height: self.prop_f64("height").unwrap_or(0.0) as f32,
                color: self.prop_str("color"),
            },
            "editable_text" => ViewKind::EditableText {
                content: self.prop_str("content").unwrap_or_default(),
                field: self
                    .prop_str("field")
                    .unwrap_or_else(|| "content".to_string()),
            },
            "image" => ViewKind::Image {
                path: self.prop_str("path").unwrap_or_default(),
                alt: self.prop_str("alt").unwrap_or_default(),
                width: self.prop_f64("width").map(|v| v as f32),
                height: self.prop_f64("height").map(|v| v as f32),
            },

            // Layout containers
            "row" => ViewKind::Row {
                gap: self.prop_f64("gap").unwrap_or(8.0) as f32,
                children: snap_children(),
            },
            "section" => ViewKind::Section {
                title: self.prop_str("title").unwrap_or_default(),
                children: snap_children(),
            },
            "column" => ViewKind::Column {
                gap: self.prop_f64("gap").unwrap_or(0.0) as f32,
                children: snap_children(),
            },
            "query_result" => ViewKind::QueryResult {
                children: snap_children(),
            },
            "tree_item" => ViewKind::TreeItem {
                depth: self.prop_f64("depth").unwrap_or(0.0) as usize,
                has_children: self.prop_bool("has_children").unwrap_or(false),
                children: snap_children(),
            },

            // Collections — the registered layout name selects which
            // `ViewKind` variant we serialize to. Layouts whose name doesn't
            // match a built-in `ViewKind` variant fall through to a generic
            // `Column` snapshot; that's serialization-only fallback (the
            // streaming runtime + platform renderers still see the real
            // layout via `view.layout()`).
            n if crate::collection_layout::is_layout(n) => {
                if let Some(ref view) = self.collection {
                    let children = match resolve_block {
                        Some(rb) => view.snapshot_resolved(rb),
                        None => view.snapshot(),
                    };
                    match view.layout().as_ref().map(|v| v.name()) {
                        Some("tree") => ViewKind::Tree { children },
                        Some("outline") => ViewKind::Outline { children },
                        Some("table") => ViewKind::Table { children },
                        Some("list") => ViewKind::List {
                            gap: view.layout().as_ref().map(|v| v.gap).unwrap_or(0.0),
                            children,
                        },
                        Some("columns") => ViewKind::Columns {
                            gap: view.layout().as_ref().map(|v| v.gap).unwrap_or(0.0),
                            children,
                        },
                        _ => ViewKind::Column { gap: 0.0, children },
                    }
                } else {
                    ViewKind::Column {
                        gap: 0.0,
                        children: snap_children(),
                    }
                }
            }

            // Elements
            "source_block" => ViewKind::SourceBlock {
                language: self
                    .prop_str("language")
                    .unwrap_or_else(|| "text".to_string()),
                content: self.prop_str("content").unwrap_or_default(),
                name: self.prop_str("name").unwrap_or_default(),
                editable: self.prop_bool("editable").unwrap_or(false),
            },
            "source_editor" => ViewKind::SourceEditor {
                language: self
                    .prop_str("language")
                    .unwrap_or_else(|| "text".to_string()),
                content: self.prop_str("content").unwrap_or_default(),
            },
            "block_operations" => ViewKind::BlockOperations {
                operations: self.prop_str("operations").unwrap_or_default(),
            },
            "state_toggle" => ViewKind::StateToggle {
                field: self
                    .prop_str("field")
                    .unwrap_or_else(|| "task_state".to_string()),
                current: self.prop_str("current").unwrap_or_default(),
                label: self.prop_str("label").unwrap_or_default(),
                states: self.prop_str("states").unwrap_or_default(),
            },
            "expand_toggle" => {
                let target_id = self.prop_str("target_id").unwrap_or_default();
                let is_expanded = self.expanded.as_ref().map_or(false, |m| m.get());
                let header_children = snap_children();
                let all_children = if is_expanded {
                    if let Some(ref slot) = self.slot {
                        let content_vm = match resolve_block {
                            Some(rb) => slot.snapshot_resolved(rb),
                            None => slot.snapshot(),
                        };
                        let mut items = header_children.items;
                        items.push(content_vm);
                        LazyChildren::fully_materialized(items)
                    } else {
                        header_children
                    }
                } else {
                    header_children
                };
                ViewKind::ExpandToggle {
                    target_id,
                    expanded: is_expanded,
                    children: all_children,
                }
            }
            "pref_field" => ViewKind::PrefField {
                key: self.prop_str("key").unwrap_or_default(),
                pref_type: self.prop_str("pref_type").unwrap_or_default(),
                value: self.prop_value("value").unwrap_or(Value::Null),
                requires_restart: self.prop_bool("requires_restart").unwrap_or(false),
                locked: self.prop_bool("locked").unwrap_or(false),
                options: match self.prop_value("options") {
                    Some(Value::Array(arr)) => arr.clone(),
                    _ => vec![],
                },
                children: snap_children(),
            },
            "table_row" => ViewKind::TableRow {
                data: self.data.get_cloned(),
            },

            // Wrappers
            "focusable" => ViewKind::Focusable {
                child: Box::new(self.children.first().map(|c| snap(c)).unwrap_or_default()),
            },
            "selectable" => ViewKind::Selectable {
                child: Box::new(self.children.first().map(|c| snap(c)).unwrap_or_default()),
            },
            "draggable" => ViewKind::Draggable {
                child: Box::new(self.children.first().map(|c| snap(c)).unwrap_or_default()),
            },
            "pie_menu" => ViewKind::PieMenu {
                fields: self.prop_str("fields").unwrap_or_default(),
                child: Box::new(self.children.first().map(|c| snap(c)).unwrap_or_default()),
            },
            "drop_zone" => ViewKind::DropZone {
                op_name: self
                    .prop_str("op")
                    .or_else(|| self.prop_str("op_name"))
                    .unwrap_or_else(|| "move_block".to_string()),
            },
            "view_mode_switcher" => {
                let entity_uri = self
                    .prop_str("entity_uri")
                    .map(|s| EntityUri::from_raw(&s))
                    .unwrap_or_else(|| EntityUri::from_raw("unknown"));
                let modes = self.prop_str("modes").unwrap_or_else(|| "[]".to_string());
                let content = if let Some(ref slot) = self.slot {
                    match resolve_block {
                        Some(rb) => slot.snapshot_resolved(rb),
                        None => slot.snapshot(),
                    }
                } else {
                    ViewModel::default()
                };
                ViewKind::ViewModeSwitcher {
                    entity_uri,
                    modes,
                    child: Box::new(content),
                }
            }
            "drawer" => ViewKind::Drawer {
                block_id: self.prop_str("block_id").unwrap_or_default(),
                mode: DrawerMode::from_str(self.prop_str("mode").as_deref().unwrap_or("shrink")),
                width: self.prop_f64("width").unwrap_or(300.0) as f32,
                child: Box::new(self.children.first().map(|c| snap(c)).unwrap_or_default()),
            },
            "card" => ViewKind::Card {
                accent: self.prop_str("accent").unwrap_or_default(),
                children: snap_children(),
            },
            "chat_bubble" => ViewKind::ChatBubble {
                sender: self.prop_str("sender").unwrap_or_default(),
                time: self.prop_str("time").unwrap_or_default(),
                children: snap_children(),
            },
            "collapsible" => ViewKind::Collapsible {
                header: self.prop_str("header").unwrap_or_default(),
                icon: self.prop_str("icon").unwrap_or_default(),
                children: snap_children(),
            },
            "bottom_dock" => ViewKind::BottomDock {
                children: snap_children(),
            },
            "op_button" => ViewKind::OpButton {
                op_name: self.prop_str("op_name").unwrap_or_default(),
                target_id: self.prop_str("target_id").unwrap_or_default(),
                display_name: self.prop_str("display_name").unwrap_or_default(),
            },

            // Block boundary — deferred to slot
            "live_block" => {
                let block_id_str = self.prop_str("block_id").unwrap_or_default();
                let block_id = EntityUri::parse(&block_id_str)
                    .unwrap_or_else(|_| EntityUri::block(&block_id_str));
                match resolve_block {
                    Some(resolve) => ViewKind::LiveBlock {
                        block_id: block_id.to_string(),
                        content: Box::new(resolve(&block_id)),
                    },
                    None => ViewKind::LiveBlock {
                        block_id: block_id.to_string(),
                        content: Box::new(
                            self.slot.as_ref().map(|s| s.snapshot()).unwrap_or_default(),
                        ),
                    },
                }
            }
            "live_query" => ViewKind::LiveQuery {
                content: Box::new(
                    self.slot
                        .as_ref()
                        .map(|s| match resolve_block {
                            Some(rb) => s.snapshot_resolved(rb),
                            None => s.snapshot(),
                        })
                        .unwrap_or_default(),
                ),
                compiled_sql: self.prop_str("compiled_sql"),
                query_context_id: self.prop_str("query_context_id"),
                render_expr: None, // TODO: store in props as serialized
            },
            "render_entity" => ViewKind::RenderBlock {
                content: Box::new(
                    self.slot
                        .as_ref()
                        .map(|s| match resolve_block {
                            Some(rb) => s.snapshot_resolved(rb),
                            None => s.snapshot(),
                        })
                        .unwrap_or_default(),
                ),
            },
            "error" => ViewKind::Error {
                message: self.prop_str("message").unwrap_or_default(),
            },
            "loading" => ViewKind::Loading,

            _ => ViewKind::Empty,
        }
    }
}

// ── Constructors ───────────────────────────────────────────────────────

impl Default for ReactiveViewModel {
    fn default() -> Self {
        Self {
            expr: Mutable::new(RenderExpr::FunctionCall {
                name: "empty".to_string(),
                args: vec![],
            }),
            // One-shot read-only handle for nodes built outside a CDC
            // pipeline (defaults, tests, snapshot fixtures). The wrapping
            // Mutable is dropped immediately; the cell stays alive via the
            // Arc inside `ReadOnlyMutable`. No upstream writer means no
            // updates — fine for a default-empty node.
            data: Mutable::new(Arc::new(HashMap::new())).read_only(),
            children: vec![],
            collection: None,
            slot: None,
            expanded: None,
            operations: vec![],
            triggers: vec![],
            layout_hint: LayoutHint::default(),
            props: Mutable::new(HashMap::new()),
            render_ctx: None,
            interpret_fn: None,
            subscriptions: Vec::new(),
        }
    }
}

impl ReactiveViewModel {
    /// Create a node from a widget name and properties.
    pub fn from_widget(name: &str, props: HashMap<String, Value>) -> Self {
        Self {
            expr: Mutable::new(RenderExpr::FunctionCall {
                name: name.to_string(),
                args: vec![],
            }),
            props: Mutable::new(props),
            ..Default::default()
        }
    }

    /// Construct a new node with `data` backed by a fresh one-shot
    /// read-only cell holding `entity`. No upstream writer means no
    /// updates — used by snapshot-style call sites (focus_path tests,
    /// shadow tree builders) that don't participate in the live CDC
    /// pipeline.
    pub fn with_entity(mut self, entity: Arc<DataRow>) -> Self {
        self.data = Mutable::new(entity).read_only();
        self
    }

    pub fn with_children(mut self, children: Vec<ReactiveViewModel>) -> Self {
        self.children = children.into_iter().map(Arc::new).collect();
        self
    }

    pub fn with_layout_hint(mut self, hint: LayoutHint) -> Self {
        self.layout_hint = hint;
        self
    }

    pub fn text(content: impl Into<String>) -> Self {
        let mut props = HashMap::new();
        props.insert("content".to_string(), Value::String(content.into()));
        props.insert("bold".to_string(), Value::Boolean(false));
        props.insert("size".to_string(), Value::Float(14.0));
        Self::from_widget("text", props)
    }

    pub fn live_block(block_id: EntityUri) -> Self {
        let mut props = HashMap::new();
        props.insert("block_id".to_string(), Value::String(block_id.to_string()));
        Self {
            slot: Some(ReactiveSlot::empty()),
            ..Self::from_widget("live_block", props)
        }
    }

    pub fn error(_widget: impl Into<String>, message: impl Into<String>) -> Self {
        let mut props = HashMap::new();
        props.insert("message".to_string(), Value::String(message.into()));
        Self::from_widget("error", props)
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn drawer(
        block_id: impl Into<String>,
        mode: DrawerMode,
        width: f32,
        child: ReactiveViewModel,
    ) -> Self {
        let mut props = HashMap::new();
        props.insert("block_id".to_string(), Value::String(block_id.into()));
        props.insert(
            "mode".to_string(),
            Value::String(
                match mode {
                    DrawerMode::Overlay => "overlay",
                    DrawerMode::Shrink => "shrink",
                }
                .to_string(),
            ),
        );
        props.insert("width".to_string(), Value::Float(width as f64));
        Self {
            children: vec![Arc::new(child)],
            ..Self::from_widget("drawer", props)
        }
    }

    /// Create a leaf node.
    pub fn leaf(widget: impl Into<String>, value: Value) -> Self {
        let widget = widget.into();
        let mut props = HashMap::new();
        match widget.as_str() {
            "text" => {
                props.insert(
                    "content".to_string(),
                    Value::String(value.to_display_string()),
                );
                props.insert("bold".to_string(), Value::Boolean(false));
                props.insert("size".to_string(), Value::Float(14.0));
            }
            "badge" => {
                props.insert(
                    "label".to_string(),
                    Value::String(value.to_display_string()),
                );
            }
            "icon" => {
                props.insert(
                    "name".to_string(),
                    Value::String(value.as_string().unwrap_or("circle").to_string()),
                );
                props.insert("size".to_string(), Value::Float(16.0));
            }
            "checkbox" => {
                props.insert(
                    "checked".to_string(),
                    Value::Boolean(value.as_bool().unwrap_or(false)),
                );
            }
            "editable_text" => {
                props.insert(
                    "content".to_string(),
                    Value::String(value.to_display_string()),
                );
                props.insert("field".to_string(), Value::String("content".to_string()));
            }
            _ => {
                props.insert(
                    "content".to_string(),
                    Value::String(value.to_display_string()),
                );
                props.insert("bold".to_string(), Value::Boolean(false));
                props.insert("size".to_string(), Value::Float(14.0));
            }
        }
        Self::from_widget(&widget, props)
    }

    /// Create an element node from a widget name and data row.
    pub fn element(
        widget: impl Into<String>,
        data: Arc<DataRow>,
        children: Vec<ReactiveViewModel>,
    ) -> Self {
        let widget = widget.into();
        let mut props = HashMap::new();

        // Extract properties from data row based on widget type
        match widget.as_str() {
            "source_block" => {
                props.insert(
                    "language".to_string(),
                    data.get("language")
                        .cloned()
                        .unwrap_or(Value::String("text".to_string())),
                );
                props.insert(
                    "content".to_string(),
                    data.get("content")
                        .cloned()
                        .unwrap_or(Value::String(String::new())),
                );
                props.insert(
                    "name".to_string(),
                    data.get("name")
                        .cloned()
                        .unwrap_or(Value::String(String::new())),
                );
                props.insert(
                    "editable".to_string(),
                    data.get("editable")
                        .cloned()
                        .unwrap_or(Value::Boolean(false)),
                );
            }
            "source_editor" => {
                props.insert(
                    "language".to_string(),
                    data.get("language")
                        .cloned()
                        .unwrap_or(Value::String("text".to_string())),
                );
                props.insert(
                    "content".to_string(),
                    data.get("content")
                        .cloned()
                        .unwrap_or(Value::String(String::new())),
                );
            }
            "block_operations" => {
                props.insert(
                    "operations".to_string(),
                    data.get("operations")
                        .cloned()
                        .unwrap_or(Value::String(String::new())),
                );
            }
            "state_toggle" => {
                props.insert(
                    "field".to_string(),
                    data.get("field")
                        .cloned()
                        .unwrap_or(Value::String("task_state".to_string())),
                );
                props.insert(
                    "current".to_string(),
                    data.get("current")
                        .cloned()
                        .unwrap_or(Value::String(String::new())),
                );
                props.insert(
                    "label".to_string(),
                    data.get("label")
                        .cloned()
                        .unwrap_or(Value::String(String::new())),
                );
                props.insert(
                    "states".to_string(),
                    data.get("states")
                        .cloned()
                        .unwrap_or(Value::String(String::new())),
                );
            }
            "expand_toggle" => {
                let target_id = data
                    .get("target_id")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string();
                let is_expanded = data
                    .get("expanded")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                props.insert("target_id".to_string(), Value::String(target_id));
                return Self {
                    expanded: Some(Mutable::new(is_expanded)),
                    children: children.into_iter().map(Arc::new).collect(),
                    ..Self::from_widget("expand_toggle", props)
                };
            }
            "pref_field" => {
                for key in [
                    "key",
                    "pref_type",
                    "value",
                    "requires_restart",
                    "locked",
                    "options",
                ] {
                    if let Some(v) = data.get(key) {
                        props.insert(key.to_string(), v.clone());
                    }
                }
            }
            _ => {}
        }

        Self {
            data: Mutable::new(data).read_only(),
            children: children.into_iter().map(Arc::new).collect(),
            ..Self::from_widget(&widget, props)
        }
    }

    /// Create a streaming collection node.
    #[allow(clippy::too_many_arguments)]
    pub fn streaming_collection(
        widget: &str,
        item_template: RenderExpr,
        data_source: std::sync::Arc<dyn holon_api::ReactiveRowProvider>,
        gap: f32,
        sort_key: Option<String>,
        parent_space: Option<crate::render_context::AvailableSpace>,
        child_space_fn: Option<std::sync::Arc<crate::reactive_view::ChildSpaceFn>>,
        virtual_child: Option<crate::reactive_view::VirtualChildSlot>,
        trailing_slot: Option<crate::reactive_view::TrailingSlot>,
    ) -> Self {
        if widget == "query_result" {
            return Self::from_widget("query_result", HashMap::new());
        }
        let layout = Self::widget_layout(widget, gap);
        let mut view = crate::reactive_view::ReactiveView::new_collection(
            crate::reactive_view::CollectionConfig {
                layout,
                item_template,
                sort_key,
                virtual_child,
            },
            data_source,
            parent_space,
            child_space_fn,
        );
        if let Some(slot) = trailing_slot {
            view.set_trailing_slot(slot);
        }
        Self {
            collection: Some(std::sync::Arc::new(view)),
            ..Self::from_widget(widget, HashMap::new())
        }
    }

    /// Create a static collection node.
    pub fn static_collection(widget: &str, items: Vec<ReactiveViewModel>, gap: f32) -> Self {
        if widget == "query_result" {
            return Self {
                children: items.into_iter().map(Arc::new).collect(),
                ..Self::from_widget("query_result", HashMap::new())
            };
        }
        let layout = Self::widget_layout(widget, gap);
        let view = crate::reactive_view::ReactiveView::new_static_with_layout(items, layout);
        Self {
            collection: Some(std::sync::Arc::new(view)),
            ..Self::from_widget(widget, HashMap::new())
        }
    }

    fn widget_layout(widget: &str, gap: f32) -> CollectionVariant {
        // Single source of truth: the `collection_layout` registry. Falls
        // back to a `list`-shaped variant for unknown widgets so the
        // streaming runtime stays well-typed even if a frontend forgets
        // to register a custom layout.
        CollectionVariant::from_name(widget, gap).unwrap_or_else(|| {
            CollectionVariant::from_name("list", gap)
                .expect("`list` layout is registered as a builtin")
        })
    }

    /// Create a layout node from a widget name and children.
    pub fn layout(widget: &str, children: Vec<ReactiveViewModel>) -> Self {
        if widget == "columns" {
            return Self::static_collection("columns", children, 16.0);
        }
        let mut props = HashMap::new();
        match widget {
            "row" => {
                props.insert("gap".to_string(), Value::Float(8.0));
            }
            "column" => {
                props.insert("gap".to_string(), Value::Float(0.0));
            }
            "bottom_dock" => {
                assert_eq!(
                    children.len(),
                    2,
                    "bottom_dock requires exactly 2 slots; got {}",
                    children.len()
                );
            }
            _ => {}
        }
        Self {
            children: children.into_iter().map(Arc::new).collect(),
            ..Self::from_widget(widget, props)
        }
    }

    /// Create a flat tree item with depth metadata.
    pub fn tree_item(content: ReactiveViewModel, depth: usize, has_children: bool) -> Self {
        let mut props = HashMap::new();
        props.insert("depth".to_string(), Value::Integer(depth as i64));
        props.insert("has_children".to_string(), Value::Boolean(has_children));
        Self {
            children: vec![Arc::new(content)],
            ..Self::from_widget("tree_item", props)
        }
    }
}

impl crate::render_interpreter::WithEntity for ReactiveViewModel {
    fn attach_entity(&mut self, entity: Arc<DataRow>) {
        // Replace the data field with a fresh one-shot read-only cell.
        // `attach_entity` is called by `shared_tree_build` for nested
        // children where no shared CDC handle is available — so the new
        // node carries a snapshot, not a shared signal source. Live data
        // sources should construct via `with_row_mutable` on the
        // `RenderContext` instead.
        self.data = Mutable::new(entity).read_only();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_signals::signal::Mutable;
    use holon_api::{RenderExpr, Value};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_data(task_state: &str) -> Arc<DataRow> {
        Arc::new(
            [
                ("id".to_string(), Value::String("block:1".to_string())),
                ("content".to_string(), Value::String("hello".to_string())),
                (
                    "task_state".to_string(),
                    Value::String(task_state.to_string()),
                ),
            ]
            .into_iter()
            .collect(),
        )
    }

    fn state_toggle_expr() -> RenderExpr {
        RenderExpr::FunctionCall {
            name: "state_toggle".to_string(),
            args: vec![],
        }
    }

    fn row_expr() -> RenderExpr {
        RenderExpr::FunctionCall {
            name: "row".to_string(),
            args: vec![],
        }
    }

    /// Architectural regression guard for the task-state-toggle bug.
    ///
    /// The bug used to be: a row's `state_toggle` child snapshotted the
    /// row data at build time, so CDC updates to the row never made it
    /// into the child's `data`/`props`. The fix is the one-writer
    /// architecture: `ReactiveRowSet` owns the only writable `Mutable`,
    /// every node holds a `ReadOnlyMutable` clone of the same cell, and
    /// leaves spawn signal subscriptions that re-derive their props on
    /// every per-row write.
    ///
    /// This test reproduces the contract at the data-cell level (no full
    /// builder pipeline): one writable cell shared by parent + child, a
    /// signal subscription on the child that updates its props, then
    /// flush the runtime and assert the child sees the new value.
    #[tokio::test(flavor = "current_thread")]
    async fn shared_data_cell_updates_propagate_to_state_toggle_child() {
        use futures_signals::signal::SignalExt;

        let old_data = make_data("TODO");

        // The CDC writer's cell. Only this `Mutable` ever calls `.set()`.
        let cell = Mutable::new(old_data.clone());
        let row_data = cell.read_only();
        let toggle_data = cell.read_only();

        // Build a state_toggle child whose `data` is a read-only clone of
        // the cell. Initial props mirror the row's current task_state.
        let state_toggle = Arc::new(ReactiveViewModel {
            expr: Mutable::new(state_toggle_expr()),
            data: toggle_data.clone(),
            props: Mutable::new(
                [
                    ("field".to_string(), Value::String("task_state".to_string())),
                    ("current".to_string(), Value::String("TODO".to_string())),
                    ("label".to_string(), Value::String("TODO".to_string())),
                ]
                .into_iter()
                .collect(),
            ),
            ..Default::default()
        });

        // The leaf's signal subscription — re-derives `current` and
        // `label` from the row signal on every emission. In production
        // this lives inside the `state_toggle` builder; here we set it up
        // by hand so the test stays self-contained. The `_subscription`
        // local owns the task and aborts it when the test scope ends.
        let _subscription = {
            let props = state_toggle.props.clone();
            let task = tokio::spawn(toggle_data.signal_cloned().for_each(move |row| {
                let new_state = row
                    .get("task_state")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string();
                let mut p = props.lock_mut();
                p.insert("current".to_string(), Value::String(new_state.clone()));
                p.insert("label".to_string(), Value::String(new_state));
                async {}
            }));
            DropTask::new(task)
        };

        // Build the parent row, sharing the same cell.
        let row = ReactiveViewModel {
            expr: Mutable::new(row_expr()),
            data: row_data,
            children: vec![state_toggle.clone()],
            ..Default::default()
        };

        // Simulate a CDC write — the `Mutable` is the sole writer.
        let new_data = make_data("DONE");
        cell.set(new_data.clone());

        // Yield the runtime so the subscription task gets to run.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        // Parent's data reflects the CDC write — same Arc<MutableState>.
        assert_eq!(
            row.data
                .get_cloned()
                .get("task_state")
                .and_then(|v| v.as_string())
                .unwrap(),
            "DONE",
            "parent data must reflect the CDC update via shared cell"
        );

        // Child's data also reflects it — same shared cell, no tree walk.
        let child_data = state_toggle.data.get_cloned();
        assert_eq!(
            child_data
                .get("task_state")
                .and_then(|v| v.as_string())
                .unwrap_or(""),
            "DONE",
            "child data must reflect the CDC update via shared cell"
        );

        // Child's derived props reflect it via the signal subscription.
        let child_props = state_toggle.props.get_cloned();
        assert_eq!(
            child_props
                .get("current")
                .and_then(|v| v.as_string())
                .unwrap_or(""),
            "DONE",
            "child's signal subscription must re-derive `current` from new data"
        );
    }

    #[test]
    fn click_intent_returns_none_for_node_without_click_op() {
        let node = ReactiveViewModel::default();
        assert!(node.click_intent().is_none());
    }

    #[test]
    fn click_intent_builds_from_click_triggered_op() {
        use holon_api::render_types::{OperationDescriptor, OperationWiring, Trigger};

        let mut bound = HashMap::new();
        bound.insert("region".to_string(), Value::String("main".into()));
        bound.insert("block_id".to_string(), Value::String("block:doc-42".into()));

        let mut node = ReactiveViewModel::default();
        node.operations.push(OperationWiring {
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: holon_api::EntityName::new("navigation"),
                name: "focus".into(),
                trigger: Some(Trigger::Click),
                bound_params: bound.clone(),
                ..Default::default()
            },
        });

        let intent = node
            .click_intent()
            .expect("click_intent should return Some for a click-bound node");
        assert_eq!(intent.entity_name.as_str(), "navigation");
        assert_eq!(intent.op_name, "focus");
        assert_eq!(intent.params, bound);
    }

    #[test]
    fn click_intent_ignores_keychord_triggered_ops() {
        use holon_api::render_types::{OperationDescriptor, OperationWiring, Trigger};

        let mut node = ReactiveViewModel::default();
        node.operations.push(OperationWiring {
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: holon_api::EntityName::new("block"),
                name: "cycle_task_state".into(),
                trigger: Some(Trigger::KeyChord {
                    chord: holon_api::KeyChord::new(&[holon_api::Key::Cmd, holon_api::Key::Enter]),
                }),
                ..Default::default()
            },
        });

        assert!(
            node.click_intent().is_none(),
            "key-chord-only op must not be treated as a click action"
        );
    }
}
