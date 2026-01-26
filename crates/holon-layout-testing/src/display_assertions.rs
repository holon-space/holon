use std::collections::HashMap;
use std::fmt::Write;

use holon_api::Value;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::view_model::ViewKind;
use holon_frontend::ViewModel;

// ── ReactiveViewModel helpers (F5) ───────────────────────────────────────
//
// These operate on the reactive tree directly so the PBT can drop its
// dependency on the fully-resolved `ViewModel` — closing the last
// legacy-type leak in the headless test path.

/// Walk `node` and invoke `f` on every direct child. Mirrors
/// `frontends/gpui/src/views/reactive_shell.rs::for_each_child`; kept local
/// to avoid a cross-crate dependency from holon-integration-tests into
/// holon-gpui just for this walker.
fn reactive_for_each_child(node: &ReactiveViewModel, mut f: impl FnMut(&ReactiveViewModel)) {
    for child in &node.children {
        f(child);
    }
    if let Some(ref view) = node.collection {
        let items: Vec<std::sync::Arc<ReactiveViewModel>> =
            view.items.lock_ref().iter().cloned().collect();
        for item in &items {
            f(item);
        }
    }
    if let Some(ref slot) = node.slot {
        let guard = slot.content.lock_ref();
        f(&guard);
    }
}

/// Collect every entity `id` that appears anywhere in the reactive tree.
/// Used by ClickBlock / ArrowNavigate assertions instead of the old
/// `ViewModel::collect_entity_ids`.
pub fn collect_entity_ids_reactive(node: &ReactiveViewModel) -> Vec<String> {
    fn walk(node: &ReactiveViewModel, out: &mut Vec<String>) {
        let entity = node.entity();
        if let Some(Value::String(id)) = entity.get("id") {
            out.push(id.clone());
        }
        reactive_for_each_child(node, |child| walk(child, out));
    }
    let mut out = Vec::new();
    walk(node, &mut out);
    out
}

/// Find the first StateToggle in the reactive tree whose entity `id`
/// matches `block_id`.
pub fn find_state_toggle_for_block_reactive<'a>(
    node: &'a ReactiveViewModel,
    block_id: &holon_api::EntityUri,
) -> Option<&'a ReactiveViewModel> {
    find_in_static_children(node, block_id)
}

fn find_in_static_children<'a>(
    node: &'a ReactiveViewModel,
    block_id: &holon_api::EntityUri,
) -> Option<&'a ReactiveViewModel> {
    if node.widget_name().as_deref() == Some("state_toggle") {
        let entity = node.entity();
        if entity
            .get("id")
            .and_then(|v| v.as_string())
            .map_or(false, |id| id == block_id.as_str())
        {
            return Some(node);
        }
    }
    node.children
        .iter()
        .find_map(|c| find_in_static_children(c, block_id))
}

/// Like `find_state_toggle_for_block_reactive` but also walks collection
/// items and slot content. Returns an Arc clone since collection items
/// can't be borrowed past the MutableVec guard.
pub fn find_state_toggle_deep(
    node: &ReactiveViewModel,
    block_id: &holon_api::EntityUri,
) -> Option<std::sync::Arc<ReactiveViewModel>> {
    if node.widget_name().as_deref() == Some("state_toggle") {
        let entity = node.entity();
        if entity
            .get("id")
            .and_then(|v| v.as_string())
            .map_or(false, |id| id == block_id.as_str())
        {
            return Some(std::sync::Arc::new(ReactiveViewModel {
                expr: futures_signals::signal::Mutable::new(node.expr.get_cloned()),
                data: futures_signals::signal::Mutable::new(node.data.get_cloned()).read_only(),
                props: futures_signals::signal::Mutable::new(node.props.get_cloned()),
                operations: node.operations.clone(),
                ..Default::default()
            }));
        }
    }
    for child in &node.children {
        if let Some(found) = find_state_toggle_deep(child, block_id) {
            return Some(found);
        }
    }
    if let Some(ref view) = node.collection {
        let items: Vec<std::sync::Arc<ReactiveViewModel>> =
            view.items.lock_ref().iter().cloned().collect();
        for item in &items {
            if let Some(found) = find_state_toggle_deep(item, block_id) {
                return Some(found);
            }
        }
    }
    if let Some(ref slot) = node.slot {
        let content = slot.content.lock_ref();
        if let Some(found) = find_state_toggle_deep(&content, block_id) {
            return Some(found);
        }
    }
    None
}

/// Find the nearest ancestor whose direct children include a node with
/// the given entity ID. Used to locate the collection item that would
/// receive `set_data` from the collection driver.
pub fn find_parent_of_entity_reactive<'a>(
    node: &'a ReactiveViewModel,
    entity_id: &str,
) -> Option<&'a ReactiveViewModel> {
    for child in &node.children {
        let child_id = child
            .entity()
            .get("id")
            .and_then(|v| v.as_string())
            .unwrap_or_default()
            .to_string();
        if child_id == entity_id {
            return Some(node);
        }
    }
    node.children
        .iter()
        .find_map(|c| find_parent_of_entity_reactive(c, entity_id))
}

// ── DiffableTree trait ──────────────────────────────────────────────────
//
// A uniform interface for diffing ViewModel and ReactiveViewModel trees.
// Implement once, diff any tree pair of the same type.

/// Trait for tree nodes that can be structurally compared.
pub trait DiffableTree {
    fn diff_widget_name(&self) -> String;
    fn diff_child_count(&self) -> usize;
    fn diff_child(&self, index: usize) -> Option<&Self>;
    fn diff_props(&self) -> HashMap<String, String>;
    fn diff_data_fields(&self) -> HashMap<String, String>;
}

impl DiffableTree for ViewModel {
    fn diff_widget_name(&self) -> String {
        self.widget_name().unwrap_or("(none)").to_string()
    }
    fn diff_child_count(&self) -> usize {
        self.children().len()
    }
    fn diff_child(&self, index: usize) -> Option<&Self> {
        self.children().get(index)
    }
    fn diff_props(&self) -> HashMap<String, String> {
        HashMap::new()
    }
    fn diff_data_fields(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for key in ["id", "content", "task_state"] {
            if let Some(v) = self.entity.get(key) {
                out.insert(key.to_string(), format_value(v));
            }
        }
        out
    }
}

impl DiffableTree for ReactiveViewModel {
    fn diff_widget_name(&self) -> String {
        self.widget_name().unwrap_or_else(|| "?".to_string())
    }
    fn diff_child_count(&self) -> usize {
        self.children.len()
    }
    fn diff_child(&self, index: usize) -> Option<&Self> {
        self.children.get(index).map(|arc| arc.as_ref())
    }
    fn diff_props(&self) -> HashMap<String, String> {
        self.props
            .lock_ref()
            .iter()
            .map(|(k, v)| (k.clone(), format_value(v)))
            .collect()
    }
    // Data fields are intentionally skipped for ReactiveViewModel diffing.
    // The collection driver's set_data populates parent nodes with full row
    // data, while fresh interpretation may not. This asymmetry is expected.
    // The PROPS comparison is what catches the actual bug: child widgets
    // (state_toggle, editable_text) having stale props after set_data.
    fn diff_data_fields(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

/// A single difference found between two trees.
#[derive(Debug, Clone)]
pub struct TreeDiff {
    pub path: String,
    pub kind: DiffKind,
}

#[derive(Debug, Clone)]
pub enum DiffKind {
    WidgetMismatch { actual: String, expected: String },
    ValueMismatch { actual: String, expected: String },
    ChildCountMismatch { actual: usize, expected: usize },
    OnlyInActual,
    OnlyInExpected,
}

impl std::fmt::Display for TreeDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "at {}: ", self.path)?;
        match &self.kind {
            DiffKind::WidgetMismatch { actual, expected } => {
                write!(f, "widget \"{actual}\" vs \"{expected}\"")
            }
            DiffKind::ValueMismatch { actual, expected } => {
                write!(f, "value {actual} vs {expected}")
            }
            DiffKind::ChildCountMismatch { actual, expected } => {
                write!(f, "{actual} children vs {expected}")
            }
            DiffKind::OnlyInActual => write!(f, "only in actual"),
            DiffKind::OnlyInExpected => write!(f, "only in expected"),
        }
    }
}

/// Recursively diff two trees. Works for any type implementing `DiffableTree`.
pub fn tree_diff<T: DiffableTree>(actual: &T, expected: &T) -> Vec<TreeDiff> {
    let mut diffs = Vec::new();
    diff_recursive(actual, expected, &mut String::from("root"), &mut diffs);
    diffs
}

fn diff_recursive<T: DiffableTree>(
    actual: &T,
    expected: &T,
    path: &mut String,
    diffs: &mut Vec<TreeDiff>,
) {
    let aw = actual.diff_widget_name();
    let ew = expected.diff_widget_name();
    if aw != ew {
        diffs.push(TreeDiff {
            path: path.clone(),
            kind: DiffKind::WidgetMismatch {
                actual: aw,
                expected: ew,
            },
        });
        return;
    }

    // Props
    let a_props = actual.diff_props();
    let e_props = expected.diff_props();
    let all_keys: std::collections::BTreeSet<&String> =
        a_props.keys().chain(e_props.keys()).collect();
    for key in all_keys {
        let av = a_props.get(key);
        let ev = e_props.get(key);
        if av != ev {
            diffs.push(TreeDiff {
                path: format!("{path}({aw}).{key}"),
                kind: DiffKind::ValueMismatch {
                    actual: av.cloned().unwrap_or_else(|| "(missing)".to_string()),
                    expected: ev.cloned().unwrap_or_else(|| "(missing)".to_string()),
                },
            });
        }
    }

    // Data fields
    let a_data = actual.diff_data_fields();
    let e_data = expected.diff_data_fields();
    let data_keys: std::collections::BTreeSet<&String> =
        a_data.keys().chain(e_data.keys()).collect();
    for key in data_keys {
        let av = a_data.get(key);
        let ev = e_data.get(key);
        if av != ev {
            diffs.push(TreeDiff {
                path: format!("{path}({aw}).data.{key}"),
                kind: DiffKind::ValueMismatch {
                    actual: av.cloned().unwrap_or_else(|| "(missing)".to_string()),
                    expected: ev.cloned().unwrap_or_else(|| "(missing)".to_string()),
                },
            });
        }
    }

    // Children
    let ac = actual.diff_child_count();
    let ec = expected.diff_child_count();
    if ac != ec {
        diffs.push(TreeDiff {
            path: path.clone(),
            kind: DiffKind::ChildCountMismatch {
                actual: ac,
                expected: ec,
            },
        });
    }
    let min = ac.min(ec);
    for i in 0..min {
        let prev_len = path.len();
        write!(path, "/[{i}]").unwrap();
        if let (Some(a), Some(e)) = (actual.diff_child(i), expected.diff_child(i)) {
            diff_recursive(a, e, path, diffs);
        }
        path.truncate(prev_len);
    }
    for i in min..ac {
        diffs.push(TreeDiff {
            path: format!("{path}/[{i}]"),
            kind: DiffKind::OnlyInActual,
        });
    }
    for i in min..ec {
        diffs.push(TreeDiff {
            path: format!("{path}/[{i}]"),
            kind: DiffKind::OnlyInExpected,
        });
    }
}

fn format_value(v: &holon_api::Value) -> String {
    match v {
        holon_api::Value::String(s) => format!("\"{s}\""),
        holon_api::Value::Integer(i) => i.to_string(),
        holon_api::Value::Float(f) => format!("{f}"),
        holon_api::Value::Boolean(b) => b.to_string(),
        holon_api::Value::Null => "null".to_string(),
        holon_api::Value::Object(m) => format!("{{...{} keys}}", m.len()),
        holon_api::Value::Array(a) => format!("[...{} items]", a.len()),
        holon_api::Value::DateTime(dt) => format!("\"{dt}\""),
        holon_api::Value::Json(j) => format!("{j}"),
    }
}

/// Check whether actual entity IDs are an ordered subset of expected IDs.
#[derive(Debug)]
pub struct OrderedSubsetResult {
    pub is_subset: bool,
    pub missing_from_expected: Vec<String>,
    pub out_of_order: Vec<(String, String)>,
}

pub fn is_ordered_subset(actual_ids: &[String], expected_ids: &[String]) -> OrderedSubsetResult {
    let mut missing_from_expected = Vec::new();
    let mut out_of_order = Vec::new();

    // Track which expected indices have been consumed so duplicate values
    // are matched one-to-one instead of all hitting the first occurrence.
    let mut consumed = vec![false; expected_ids.len()];
    let mut last_expected_idx: Option<usize> = None;

    for actual_id in actual_ids {
        // Find the first unconsumed match in expected_ids
        let found = expected_ids
            .iter()
            .enumerate()
            .position(|(i, e)| !consumed[i] && e == actual_id);

        match found {
            None => missing_from_expected.push(actual_id.clone()),
            Some(idx) => {
                consumed[idx] = true;
                if let Some(prev) = last_expected_idx {
                    if idx <= prev {
                        out_of_order.push((actual_id.clone(), expected_ids[prev].clone()));
                    }
                }
                last_expected_idx = Some(idx);
            }
        }
    }

    let is_subset = missing_from_expected.is_empty() && out_of_order.is_empty();
    OrderedSubsetResult {
        is_subset,
        missing_from_expected,
        out_of_order,
    }
}

/// Assert two ViewModel trees match structurally, with a nice error message on failure.
pub fn assert_display_trees_match(actual: &ViewModel, expected: &ViewModel, message: &str) {
    let diffs = tree_diff(actual, expected);
    assert!(
        diffs.is_empty(),
        "{message}\n\nFound {} difference(s):\n{}\n\n--- actual ---\n{}\n--- expected ---\n{}",
        diffs.len(),
        diffs
            .iter()
            .map(|d| format!("  {d}"))
            .collect::<Vec<_>>()
            .join("\n"),
        actual.pretty_print(0),
        expected.pretty_print(0),
    );
}

/// Extract per-row entity data from a rendered ViewModel tree.
///
/// Walks the collection children and extracts data based on node type:
/// - `TableRow { data }` → full row data
/// - `LiveBlock { block_id }` → `{"id": block_id}`
/// - Wrapper nodes (Focusable, Selectable, etc.) → unwrap and recurse
///
/// This is the "decompiler" — inverse of the shadow interpreter's rendering.
/// It extracts whatever data is structurally present, regardless of template.
pub fn extract_rendered_rows(tree: &ViewModel) -> Vec<HashMap<String, Value>> {
    tree.children()
        .iter()
        .filter_map(|child| extract_item_data(child))
        .collect()
}

fn extract_item_data(node: &ViewModel) -> Option<HashMap<String, Value>> {
    match &node.kind {
        ViewKind::TableRow { data } => Some((**data).clone()),
        ViewKind::LiveBlock { block_id, .. } => Some(HashMap::from([(
            "id".to_string(),
            Value::String(block_id.clone()),
        )])),
        // Wrapper nodes: unwrap and try the inner child
        ViewKind::Focusable { child }
        | ViewKind::Selectable { child }
        | ViewKind::Draggable { child }
        | ViewKind::PieMenu { child, .. } => extract_item_data(child),
        // Layout/container with children: try extracting from first leaf-like descendant
        ViewKind::Row { children, .. } | ViewKind::Column { children, .. } => {
            extract_from_children(&children.items)
        }
        _ => None,
    }
}

fn extract_from_children(children: &[ViewModel]) -> Option<HashMap<String, Value>> {
    // Walk children looking for data-bearing nodes
    let mut data = HashMap::new();
    for child in children {
        match &child.kind {
            ViewKind::Text { content, .. } => {
                // A text node rendering a column value
                data.insert("content".to_string(), Value::String(content.clone()));
            }
            ViewKind::EditableText { content, field } => {
                data.insert(field.clone(), Value::String(content.clone()));
            }
            ViewKind::LiveBlock { block_id, .. } => {
                data.insert("id".to_string(), Value::String(block_id.clone()));
            }
            ViewKind::StateToggle { field, current, .. } => {
                data.insert(field.clone(), Value::String(current.clone()));
            }
            _ => {}
        }
    }
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

/// Collect all nodes in the tree matching a predicate.
pub fn collect_nodes<'a>(
    node: &'a ViewModel,
    predicate: &dyn Fn(&ViewModel) -> bool,
) -> Vec<&'a ViewModel> {
    let mut result = Vec::new();
    collect_nodes_recursive(node, predicate, &mut result);
    result
}

fn collect_nodes_recursive<'a>(
    node: &'a ViewModel,
    predicate: &dyn Fn(&ViewModel) -> bool,
    result: &mut Vec<&'a ViewModel>,
) {
    if predicate(node) {
        result.push(node);
    }
    for child in node.children() {
        collect_nodes_recursive(child, predicate, result);
    }
}

/// Collect all EditableText nodes in the tree.
pub fn collect_editable_text_nodes(node: &ViewModel) -> Vec<&ViewModel> {
    collect_nodes(node, &|n| matches!(&n.kind, ViewKind::EditableText { .. }))
}

/// Collect all StateToggle nodes in the tree.
pub fn collect_state_toggle_nodes(node: &ViewModel) -> Vec<&ViewModel> {
    collect_nodes(node, &|n| matches!(&n.kind, ViewKind::StateToggle { .. }))
}

/// Find the first StateToggle node whose entity `id` matches `block_id`.
pub fn find_state_toggle_for_block<'a>(
    node: &'a ViewModel,
    block_id: &holon_api::EntityUri,
) -> Option<&'a ViewModel> {
    if matches!(&node.kind, ViewKind::StateToggle { .. }) {
        if node
            .entity
            .get("id")
            .and_then(|v| v.as_string())
            .map_or(false, |id| id == block_id.as_str())
        {
            return Some(node);
        }
    }
    node.children()
        .iter()
        .find_map(|c| find_state_toggle_for_block(c, block_id))
}

/// Count EditableText nodes that have operations but are missing triggers.
/// Returns (total_editable_with_ops, missing_triggers_count).
pub fn count_editables_missing_triggers(node: &ViewModel) -> (usize, usize) {
    let editables = collect_editable_text_nodes(node);
    let with_ops: Vec<_> = editables
        .iter()
        .filter(|n| !n.operations.is_empty())
        .collect();
    let missing = with_ops.iter().filter(|n| n.triggers.is_empty()).count();
    (with_ops.len(), missing)
}

/// Count nodes with widget_name == "error" in a ViewModel tree.
pub fn count_error_nodes(node: &ViewModel) -> usize {
    let self_count = if node.widget_name() == Some("error") {
        1
    } else {
        0
    };
    self_count + node.children().iter().map(count_error_nodes).sum::<usize>()
}

/// Collect a one-line summary of every Error node in the tree. Used by
/// inv14b to surface the actual error messages without needing to dump
/// the full ViewModel.
pub fn collect_error_node_summaries(node: &ViewModel) -> Vec<String> {
    let mut out = Vec::new();
    walk_error_nodes(node, &mut out);
    out
}

fn walk_error_nodes(node: &ViewModel, out: &mut Vec<String>) {
    if node.widget_name() == Some("error") {
        let entity_id = node.entity_id().unwrap_or("<no entity_id>");
        // The error message lives in `kind: ViewKind::Error { message }`,
        // not in `entity` — `ViewModel::error()` discards the entity row.
        let message = match &node.kind {
            ViewKind::Error { message } => message.clone(),
            _ => "<not Error variant>".to_string(),
        };
        out.push(format!("entity={entity_id} message={message}"));
    }
    for child in node.children() {
        walk_error_nodes(child, out);
    }
}

#[cfg(test)]
mod tests {
    use holon_api::Value;
    use holon_frontend::ViewModel;

    use super::*;

    #[test]
    fn identical_trees_produce_no_diffs() {
        let tree = ViewModel::layout(
            "column",
            vec![ViewModel::leaf("text", Value::String("hello".into()))],
        );
        assert!(tree_diff(&tree, &tree).is_empty());
    }

    #[test]
    fn widget_mismatch_detected() {
        let a = ViewModel::leaf("text", Value::String("x".into()));
        let b = ViewModel::leaf("badge", Value::String("x".into()));
        let diffs = tree_diff(&a, &b);
        assert_eq!(diffs.len(), 1);
        assert!(matches!(diffs[0].kind, DiffKind::WidgetMismatch { .. }));
    }

    #[test]
    fn child_count_mismatch_detected() {
        let a = ViewModel::layout("row", vec![ViewModel::empty(), ViewModel::empty()]);
        let b = ViewModel::layout("row", vec![ViewModel::empty()]);
        let diffs = tree_diff(&a, &b);
        assert!(diffs.iter().any(|d| matches!(
            d.kind,
            DiffKind::ChildCountMismatch {
                actual: 2,
                expected: 1
            }
        )));
    }

    #[test]
    fn ordered_subset_happy() {
        let actual = vec!["a".into(), "c".into()];
        let expected = vec!["a".into(), "b".into(), "c".into()];
        let result = is_ordered_subset(&actual, &expected);
        assert!(result.is_subset);
    }

    #[test]
    fn ordered_subset_out_of_order() {
        let actual = vec!["c".into(), "a".into()];
        let expected = vec!["a".into(), "b".into(), "c".into()];
        let result = is_ordered_subset(&actual, &expected);
        assert!(!result.is_subset);
        assert!(!result.out_of_order.is_empty());
    }

    #[test]
    fn ordered_subset_missing() {
        let actual = vec!["a".into(), "z".into()];
        let expected = vec!["a".into(), "b".into()];
        let result = is_ordered_subset(&actual, &expected);
        assert!(!result.is_subset);
        assert_eq!(result.missing_from_expected, vec!["z"]);
    }
}
