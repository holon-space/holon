//! Focus-path input routing — walks the `ReactiveViewModel` tree on demand
//! instead of maintaining a separate flattened index.
//!
//! A `FocusPath` is the ancestor chain from root to focused entity, built
//! by DFS on focus change (infrequent). Bubbling walks the path backwards,
//! lazily building navigators when it hits collection nodes.
//!
//! Replaces `IncrementalShadowIndex` — no global mutable index, no splice
//! arithmetic, no stale entries.

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::render_types::OperationWiring;

use crate::input::{InputAction, KeyChord, WidgetInput};
use crate::navigation::{CollectionNavigator, ListNavigator, TreeNavigator};
use crate::reactive_view_model::ReactiveViewModel;

// ── FocusPath ──────────────────────────────────────────────────────────

/// An ancestor chain from root to focused entity.
///
/// Built by DFS search through the `ReactiveViewModel` tree. The last
/// entry is the focused node, the first is the root.
pub struct FocusPath {
    path: Vec<FocusPathEntry>,
}

struct FocusPathEntry {
    node: Arc<ReactiveViewModel>,
    widget_name: Option<String>,
}

impl FocusPath {
    /// Walk the path backwards (from focused node toward root), checking
    /// each ancestor for a handler. Returns the first matching `InputAction`.
    #[tracing::instrument(level = "debug", skip_all, fields(entity_id))]
    pub fn bubble_input(&self, entity_id: &str, input: &WidgetInput) -> Option<InputAction> {
        for entry in self.path.iter().rev() {
            if let Some(action) = try_handle(entry, entity_id, input) {
                return Some(action);
            }
        }
        None
    }

    /// The entity IDs along the path (root to focused).
    pub fn entity_ids(&self) -> Vec<Option<String>> {
        self.path
            .iter()
            .map(|e| resolve_entity_id(&e.node))
            .collect()
    }

    /// Index of the deepest collection node in the path, if any.
    fn deepest_collection_index(&self) -> Option<usize> {
        self.path
            .iter()
            .rposition(|e| is_collection_widget(e.widget_name.as_deref()))
    }
}

/// Build a `FocusPath` from `root` to the node with `entity_id`.
///
/// DFS through the tree, following `live_block` slot content transparently.
/// Returns `None` if `entity_id` is not found.
pub fn build_focus_path(root: &Arc<ReactiveViewModel>, entity_id: &str) -> Option<FocusPath> {
    let mut stack: Vec<Arc<ReactiveViewModel>> = Vec::new();
    if dfs_find(root, entity_id, &mut stack) {
        let path = stack
            .into_iter()
            .map(|node| {
                let widget_name = node.widget_name();
                FocusPathEntry { node, widget_name }
            })
            .collect();
        Some(FocusPath { path })
    } else {
        None
    }
}

/// DFS search returning the first node whose entity id matches `entity_id`.
///
/// Same traversal as `build_focus_path` (live-block slots transparent), but
/// returns just the leaf node — useful for callers that want to read
/// `click_intent()`, `prop_*`, or `data` from a specific entity without
/// needing the bubble-up ancestor chain.
pub fn find_node_by_id(
    root: &Arc<ReactiveViewModel>,
    entity_id: &str,
) -> Option<Arc<ReactiveViewModel>> {
    let mut stack: Vec<Arc<ReactiveViewModel>> = Vec::new();
    if dfs_find(root, entity_id, &mut stack) {
        stack.pop()
    } else {
        None
    }
}

/// DFS walk: visit `node`, then children, collection items, slot. Used by
/// `UserDriver::drop_entity` to scan for `draggable` / `drop_zone` widgets
/// across the rendered tree, including across the LiveBlock slot boundary.
pub fn walk_tree<F: FnMut(&ReactiveViewModel)>(node: &ReactiveViewModel, f: &mut F) {
    f(node);
    for child in &node.children {
        walk_tree(child, f);
    }
    if let Some(view) = &node.collection {
        let items: Vec<_> = view.items.lock_ref().iter().cloned().collect();
        for item in &items {
            walk_tree(item, f);
        }
    }
    if let Some(slot) = &node.slot {
        let inner = slot.content.get_cloned();
        walk_tree(&inner, f);
    }
}

/// One-shot: find the node bound to `entity_id` and return its
/// `click_intent()` if it has a click-triggered operation.
///
/// Mirrors `bubble_input_oneshot` for the click path: works with a bare
/// `&ReactiveViewModel` (no Arc needed), build-once-use-once. Used by
/// `UserDriver::click_entity_with_tree` to dispatch the bound action of a
/// `selectable` (or any widget that wires `Trigger::Click` on its
/// operations) without exposing internal node handles to the driver.
pub fn find_click_intent_oneshot(
    root: &ReactiveViewModel,
    entity_id: &str,
) -> Option<crate::operations::OperationIntent> {
    fn walk(
        node: &ReactiveViewModel,
        entity_id: &str,
    ) -> Option<crate::operations::OperationIntent> {
        if resolve_entity_id(node).as_deref() == Some(entity_id) {
            return node.click_intent();
        }
        for child in collect_children(node) {
            if let Some(intent) = walk(&child, entity_id) {
                return Some(intent);
            }
        }
        None
    }
    walk(root, entity_id)
}

/// Static-snapshot variant of `find_click_intent_oneshot` for `ViewModel` trees.
///
/// Used when the live reactive tree's `live_block` slots haven't been filled
/// yet (a common headless-test situation where no consumer drains per-block
/// streams into slots). The caller obtains a fully-resolved `ViewModel` via
/// `BuilderServices::snapshot_resolved`, which recursively interprets every
/// nested block, then walks it here. The `OperationWiring` info is identical
/// across both representations, so the resulting `OperationIntent` matches
/// what GPUI would dispatch on a real click.
pub fn find_click_intent_in_view_model(
    root: &crate::view_model::ViewModel,
    entity_id: &str,
) -> Option<crate::operations::OperationIntent> {
    fn walk(
        node: &crate::view_model::ViewModel,
        entity_id: &str,
    ) -> Option<crate::operations::OperationIntent> {
        if node.entity_id() == Some(entity_id) {
            if let Some(op) = node
                .operations
                .iter()
                .find(|ow| ow.descriptor.is_click_triggered())
            {
                return Some(crate::operations::OperationIntent::new(
                    op.descriptor.entity_name.clone(),
                    op.descriptor.name.clone(),
                    op.descriptor.bound_params.clone(),
                ));
            }
        }
        for child in node.children() {
            if let Some(intent) = walk(child, entity_id) {
                return Some(intent);
            }
        }
        None
    }
    walk(root, entity_id)
}

/// Build a `FocusPath` across block boundaries for the headless path.
///
/// `live_block` nodes in the root tree have empty slots (content is populated
/// independently per block). `block_contents` maps block_id → latest
/// `ReactiveViewModel` for that block's content.
pub fn build_focus_path_cross_block(
    root_content: &Arc<ReactiveViewModel>,
    block_contents: &HashMap<String, Arc<ReactiveViewModel>>,
    entity_id: &str,
) -> Option<FocusPath> {
    let mut stack: Vec<Arc<ReactiveViewModel>> = Vec::new();
    if dfs_find_cross_block(root_content, block_contents, entity_id, &mut stack) {
        let path = stack
            .into_iter()
            .map(|node| {
                let widget_name = node.widget_name();
                FocusPathEntry { node, widget_name }
            })
            .collect();
        Some(FocusPath { path })
    } else {
        None
    }
}

/// One-shot: find entity by DFS and bubble input through ancestors.
///
/// Combines DFS search + bubbling in a single recursive pass. Works with
/// a bare `&ReactiveViewModel` reference (no `Arc` needed). Used by
/// `UserDriver` trait defaults that build-once, use-once, discard.
pub fn bubble_input_oneshot(
    root: &ReactiveViewModel,
    entity_id: &str,
    input: &WidgetInput,
) -> Option<InputAction> {
    match dfs_and_bubble(root, entity_id, input) {
        DfsResult::Handled(action) => Some(action),
        _ => None,
    }
}

enum DfsResult {
    /// Entity not found in this subtree.
    NotFound,
    /// Entity found but no ancestor handled the input.
    Found,
    /// Entity found and input was handled.
    Handled(InputAction),
}

/// Recursive DFS that bubbles on the way back up.
///
/// When the target entity is found, returns `Found`. Each ancestor frame
/// then tries to handle the input. The first match returns `Handled`.
fn dfs_and_bubble(node: &ReactiveViewModel, entity_id: &str, input: &WidgetInput) -> DfsResult {
    if resolve_entity_id(node).as_deref() == Some(entity_id) {
        if let Some(action) = try_handle_node(node, entity_id, input) {
            return DfsResult::Handled(action);
        }
        return DfsResult::Found;
    }

    for child in collect_children(node) {
        match dfs_and_bubble(&child, entity_id, input) {
            DfsResult::Handled(action) => return DfsResult::Handled(action),
            DfsResult::Found => {
                if let Some(action) = try_handle_node(node, entity_id, input) {
                    return DfsResult::Handled(action);
                }
                return DfsResult::Found;
            }
            DfsResult::NotFound => continue,
        }
    }

    DfsResult::NotFound
}

fn try_handle_node(
    node: &ReactiveViewModel,
    origin_id: &str,
    input: &WidgetInput,
) -> Option<InputAction> {
    match input {
        WidgetInput::Navigate { direction, hint } => {
            let wn = node.widget_name()?;
            if !is_collection_widget(Some(&wn)) {
                return None;
            }
            let children = collect_children(node);
            let navigator = build_navigator(&wn, &children)?;
            let target = navigator.navigate(origin_id, *direction, hint)?;
            Some(InputAction::Focus {
                block_id: target.block_id,
                placement: target.placement,
            })
        }
        WidgetInput::KeyChord { keys } => {
            let chord = KeyChord(keys.clone());
            let op = node
                .operations
                .iter()
                .find(|ow: &&OperationWiring| ow.descriptor.key_chord() == Some(&chord))?;
            Some(InputAction::ExecuteOperation {
                entity_name: op.descriptor.entity_name.to_string(),
                operation: op.descriptor.clone(),
                entity_id: origin_id.to_string(),
            })
        }
    }
}

/// Collect all entity IDs reachable from `root` via DFS.
/// Standalone utility replacing `IncrementalShadowIndex::entity_ids()`.
pub fn collect_all_entity_ids(root: &ReactiveViewModel) -> Vec<String> {
    let mut ids = Vec::new();
    collect_ids_dfs(root, &mut ids);
    ids
}

fn collect_ids_dfs(node: &ReactiveViewModel, ids: &mut Vec<String>) {
    if let Some(id) = resolve_entity_id(node) {
        ids.push(id);
    }
    for child in collect_children(node) {
        collect_ids_dfs(&child, ids);
    }
}

// ── InputRouter ────────────────────────────────────────────────────────

/// Resolves a `live_block`'s nested content tree by block id.
///
/// Production GPUI's `nav.set_root` only carries the shallow root tree —
/// `live_block` widgets have empty slots because their content is owned by
/// nested `ReactiveShell` entities, not by the reactive tree. Without a
/// resolver, `bubble_input` from a focused widget *inside* a live block
/// (every Main-panel block) walks past the empty slot and never finds the
/// entity → silent no-op for chord ops (Tab/Shift+Tab/Enter/Alt+Up/Alt+Down).
///
/// The resolver is the bridge: when DFS hits a `live_block`, it asks the
/// resolver for the block's current `ReactiveViewModel` and continues into
/// it. Production wires this to `ReactiveEngine::snapshot_reactive`.
pub type LiveBlockResolver = Arc<dyn Fn(&str) -> Option<Arc<ReactiveViewModel>> + Send + Sync>;

/// Frontend-agnostic input router. Caches focus path, rebuilds on focus change.
///
/// Any frontend (GPUI, MCP, headless tests) can construct one and call
/// `bubble_input`. The root tree is set on structural changes; the focus
/// path is rebuilt lazily when the focused entity changes.
pub struct InputRouter {
    root: std::sync::RwLock<Option<Arc<ReactiveViewModel>>>,
    cached: std::sync::RwLock<Option<CachedFocusPath>>,
    block_resolver: std::sync::RwLock<Option<LiveBlockResolver>>,
}

struct CachedFocusPath {
    entity_id: String,
    focus_path: FocusPath,
}

impl InputRouter {
    pub fn new() -> Self {
        Self {
            root: std::sync::RwLock::new(None),
            cached: std::sync::RwLock::new(None),
            block_resolver: std::sync::RwLock::new(None),
        }
    }

    /// Update the root tree. Invalidates the cached focus path.
    pub fn set_root(&self, root_tree: Arc<ReactiveViewModel>) {
        *self.root.write().unwrap() = Some(root_tree);
        *self.cached.write().unwrap() = None;
    }

    /// Install a resolver for `live_block` widgets. See `LiveBlockResolver`
    /// docs for why this is necessary in production. Headless tests don't
    /// need it — they use `HeadlessInputRouter` (per-block content map).
    pub fn set_block_resolver(&self, resolver: LiveBlockResolver) {
        *self.block_resolver.write().unwrap() = Some(resolver);
        *self.cached.write().unwrap() = None;
    }

    /// Route input for `entity_id`. Rebuilds the focus path if the entity
    /// changed since the last call.
    ///
    /// If the result is `Focus { block_id }` (navigation), the cache is
    /// updated: the common prefix up to the collection parent is kept, and
    /// only the segment from collection → new target is rebuilt via DFS.
    #[tracing::instrument(level = "debug", skip_all, fields(entity_id))]
    pub fn bubble_input(&self, entity_id: &str, input: &WidgetInput) -> Option<InputAction> {
        self.ensure_focus_path(entity_id);
        let guard = self.cached.read().unwrap();
        let cached = guard.as_ref()?;
        let result = cached.focus_path.bubble_input(entity_id, input);

        if let Some(InputAction::Focus { ref block_id, .. }) = result {
            drop(guard);
            self.update_cache_for_navigation(block_id);
        }

        result
    }

    /// Diagnostic: describe the current root tree.
    pub fn has_root(&self) -> bool {
        self.root.read().unwrap().is_some()
    }

    /// Diagnostic: describe the current root tree.
    pub fn describe(&self) -> String {
        let guard = self.root.read().unwrap();
        match guard.as_ref() {
            Some(root) => describe_tree(root, 0),
            None => "InputRouter: no root set".to_string(),
        }
    }

    /// Diagnostic: describe the cached focus path (if any).
    pub fn describe_focus_path(&self) -> String {
        let guard = self.cached.read().unwrap();
        match guard.as_ref() {
            Some(cached) => {
                use std::fmt::Write;
                let mut out = String::new();
                writeln!(
                    out,
                    "Focus path to '{}' ({} ancestors):",
                    cached.entity_id,
                    cached.focus_path.path.len()
                )
                .ok();
                for (i, entry) in cached.focus_path.path.iter().enumerate() {
                    let widget = entry.widget_name.as_deref().unwrap_or("?");
                    let eid = resolve_entity_id(&entry.node).unwrap_or_else(|| "-".to_string());
                    let is_collection = if is_collection_widget(entry.widget_name.as_deref()) {
                        " [NAV]"
                    } else {
                        ""
                    };
                    let ops = if entry.node.operations.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " ops=[{}]",
                            entry
                                .node
                                .operations
                                .iter()
                                .map(|o| o.descriptor.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                    writeln!(out, "  {i}: {widget} id={eid}{is_collection}{ops}").ok();
                }
                out
            }
            None => "No cached focus path".to_string(),
        }
    }

    fn ensure_focus_path(&self, entity_id: &str) {
        {
            let guard = self.cached.read().unwrap();
            if let Some(ref cached) = *guard {
                if cached.entity_id == entity_id {
                    return;
                }
            }
        }
        let root_guard = self.root.read().unwrap();
        if let Some(ref root) = *root_guard {
            let resolver_guard = self.block_resolver.read().unwrap();
            let resolver = resolver_guard.as_ref();
            let fp = match resolver {
                Some(r) => build_focus_path_with_resolver(root, entity_id, r.as_ref()),
                None => build_focus_path(root, entity_id),
            };
            if let Some(fp) = fp {
                *self.cached.write().unwrap() = Some(CachedFocusPath {
                    entity_id: entity_id.to_string(),
                    focus_path: fp,
                });
            }
        }
    }

    /// After navigation returns `Focus { block_id }`, try to reuse the
    /// common prefix of the cached path (up to the collection parent) and
    /// only DFS from there to find the new target. Falls back to full
    /// rebuild if the optimization doesn't apply.
    fn update_cache_for_navigation(&self, new_entity_id: &str) {
        let root_guard = self.root.read().unwrap();
        let Some(ref root) = *root_guard else { return };

        let resolver_guard = self.block_resolver.read().unwrap();
        let resolver = resolver_guard.as_ref();
        let mut cache_guard = self.cached.write().unwrap();

        // Try to reuse the prefix up to the deepest collection node.
        if let Some(ref cached) = *cache_guard {
            if let Some(col_idx) = cached.focus_path.deepest_collection_index() {
                let collection_node = &cached.focus_path.path[col_idx].node;
                let mut sub_stack: Vec<Arc<ReactiveViewModel>> = Vec::new();
                let found = match resolver {
                    Some(r) => dfs_find_with_resolver(
                        collection_node,
                        new_entity_id,
                        r.as_ref(),
                        &mut sub_stack,
                    ),
                    None => dfs_find(collection_node, new_entity_id, &mut sub_stack),
                };
                if found {
                    let mut new_path: Vec<FocusPathEntry> = cached.focus_path.path[..col_idx]
                        .iter()
                        .map(|e| FocusPathEntry {
                            node: e.node.clone(),
                            widget_name: e.widget_name.clone(),
                        })
                        .collect();
                    new_path.extend(sub_stack.into_iter().map(|node| {
                        let widget_name = node.widget_name();
                        FocusPathEntry { node, widget_name }
                    }));
                    *cache_guard = Some(CachedFocusPath {
                        entity_id: new_entity_id.to_string(),
                        focus_path: FocusPath { path: new_path },
                    });
                    return;
                }
            }
        }

        // Fallback: full DFS from root.
        let fp = match resolver {
            Some(r) => build_focus_path_with_resolver(root, new_entity_id, r.as_ref()),
            None => build_focus_path(root, new_entity_id),
        };
        if let Some(fp) = fp {
            *cache_guard = Some(CachedFocusPath {
                entity_id: new_entity_id.to_string(),
                focus_path: fp,
            });
        }
    }
}

// ── DFS search ─────────────────────────────────────────────────────────

/// DFS through the tree, pushing ancestors onto `stack`. Returns `true`
/// if `entity_id` was found. On return, `stack` contains the path from
/// root to the found node (inclusive).
fn dfs_find(
    node: &Arc<ReactiveViewModel>,
    entity_id: &str,
    stack: &mut Vec<Arc<ReactiveViewModel>>,
) -> bool {
    stack.push(node.clone());

    if resolve_entity_id(node).as_deref() == Some(entity_id) {
        return true;
    }

    for child in collect_children(node) {
        if dfs_find(&child, entity_id, stack) {
            return true;
        }
    }

    stack.pop();
    false
}

/// DFS that crosses block boundaries using the `block_contents` map.
/// When hitting a `live_block` node, looks up the block's content in the
/// map and continues DFS into it.
fn dfs_find_cross_block(
    node: &Arc<ReactiveViewModel>,
    block_contents: &HashMap<String, Arc<ReactiveViewModel>>,
    entity_id: &str,
    stack: &mut Vec<Arc<ReactiveViewModel>>,
) -> bool {
    stack.push(node.clone());

    if resolve_entity_id(node).as_deref() == Some(entity_id) {
        return true;
    }

    // If this is a live_block, look up the block's content.
    if node.widget_name().as_deref() == Some("live_block") {
        if let Some(block_id) = node.prop_str("block_id") {
            if let Some(content) = block_contents.get(&block_id) {
                if dfs_find_cross_block(content, block_contents, entity_id, stack) {
                    return true;
                }
            }
        }
        stack.pop();
        return false;
    }

    for child in collect_children_arcs(node) {
        if dfs_find_cross_block(&child, block_contents, entity_id, stack) {
            return true;
        }
    }

    stack.pop();
    false
}

/// Build a `FocusPath` using a `LiveBlockResolver` to cross live_block
/// boundaries. Mirrors `build_focus_path_cross_block` but instead of a
/// pre-built map, asks the resolver on demand. Used by production GPUI
/// where live_block slots in `nav.set_root`'s tree are empty.
pub fn build_focus_path_with_resolver(
    root: &Arc<ReactiveViewModel>,
    entity_id: &str,
    resolver: &(dyn Fn(&str) -> Option<Arc<ReactiveViewModel>> + Send + Sync),
) -> Option<FocusPath> {
    let mut stack: Vec<Arc<ReactiveViewModel>> = Vec::new();
    if dfs_find_with_resolver(root, entity_id, resolver, &mut stack) {
        let path = stack
            .into_iter()
            .map(|node| {
                let widget_name = node.widget_name();
                FocusPathEntry { node, widget_name }
            })
            .collect();
        Some(FocusPath { path })
    } else {
        None
    }
}

fn dfs_find_with_resolver(
    node: &Arc<ReactiveViewModel>,
    entity_id: &str,
    resolver: &(dyn Fn(&str) -> Option<Arc<ReactiveViewModel>> + Send + Sync),
    stack: &mut Vec<Arc<ReactiveViewModel>>,
) -> bool {
    stack.push(node.clone());

    if resolve_entity_id(node).as_deref() == Some(entity_id) {
        return true;
    }

    if node.widget_name().as_deref() == Some("live_block") {
        if let Some(block_id) = node.prop_str("block_id") {
            if let Some(content) = resolver(&block_id) {
                if dfs_find_with_resolver(&content, entity_id, resolver, stack) {
                    return true;
                }
            }
        }
        stack.pop();
        return false;
    }

    for child in collect_children_arcs(node) {
        if dfs_find_with_resolver(&child, entity_id, resolver, stack) {
            return true;
        }
    }

    stack.pop();
    false
}

// ── Input handling ─────────────────────────────────────────────────────

fn try_handle(entry: &FocusPathEntry, origin_id: &str, input: &WidgetInput) -> Option<InputAction> {
    match input {
        WidgetInput::Navigate { direction, hint } => {
            try_navigate(entry, origin_id, *direction, hint)
        }
        WidgetInput::KeyChord { keys } => try_keychord(entry, origin_id, keys),
    }
}

fn try_navigate(
    entry: &FocusPathEntry,
    origin_id: &str,
    direction: crate::navigation::NavDirection,
    hint: &crate::navigation::CursorHint,
) -> Option<InputAction> {
    let wn = entry.widget_name.as_deref()?;
    if !is_collection_widget(Some(wn)) {
        return None;
    }
    let children = collect_children(&entry.node);
    let navigator = build_navigator(wn, &children)?;
    let target = navigator.navigate(origin_id, direction, hint)?;
    Some(InputAction::Focus {
        block_id: target.block_id,
        placement: target.placement,
    })
}

fn try_keychord(
    entry: &FocusPathEntry,
    origin_id: &str,
    keys: &std::collections::BTreeSet<crate::input::Key>,
) -> Option<InputAction> {
    let chord = KeyChord(keys.clone());
    let op_match = entry
        .node
        .operations
        .iter()
        .find(|ow: &&OperationWiring| ow.descriptor.key_chord() == Some(&chord));

    if std::env::var("HOLON_DEBUG_CHORD").is_ok() {
        let ops: Vec<String> = entry
            .node
            .operations
            .iter()
            .map(|ow| {
                format!(
                    "{}::{}{}",
                    ow.descriptor.entity_name,
                    ow.descriptor.name,
                    if let Some(kc) = ow.descriptor.key_chord() {
                        format!(" [{kc:?}]")
                    } else {
                        String::new()
                    }
                )
            })
            .collect();
        tracing::debug!(
            "[CHORD] entry.widget={:?} origin={} chord={:?} ops=[{}] match={}",
            entry.widget_name,
            origin_id,
            chord,
            ops.join(", "),
            op_match.is_some(),
        );
    }

    let op = op_match?;
    Some(InputAction::ExecuteOperation {
        entity_name: op.descriptor.entity_name.to_string(),
        operation: op.descriptor.clone(),
        entity_id: origin_id.to_string(),
    })
}

fn is_collection_widget(name: Option<&str>) -> bool {
    matches!(
        name,
        Some("list" | "tree" | "outline" | "table" | "query_result")
    )
}

// ── Shared utilities (extracted from shadow_index.rs) ──────────────────

/// Snapshot a `ReactiveViewModel`'s direct children as a concrete `Vec`.
///
/// Traverses `children`, `collection.items`, and `slot.content`.
pub fn collect_children(node: &ReactiveViewModel) -> Vec<Arc<ReactiveViewModel>> {
    let mut result: Vec<Arc<ReactiveViewModel>> = Vec::new();

    if !node.children.is_empty() {
        result.extend(node.children.iter().cloned());
    }

    if let Some(ref view) = node.collection {
        let items: Vec<Arc<ReactiveViewModel>> = view.items.lock_ref().iter().cloned().collect();
        result.extend(items);
    }

    if let Some(ref slot) = node.slot {
        result.push(slot.content.lock_ref().clone());
    }

    result
}

/// Same as `collect_children` but takes `&Arc<ReactiveViewModel>`.
fn collect_children_arcs(node: &Arc<ReactiveViewModel>) -> Vec<Arc<ReactiveViewModel>> {
    collect_children(node.as_ref())
}

/// Resolve `entity_id` for a node. Returns the explicit ID for nodes that
/// have one, otherwise falls back to `entity().get("id")`.
pub fn resolve_entity_id(node: &ReactiveViewModel) -> Option<String> {
    if let Some(id) = node.entity_id() {
        return Some(id);
    }
    let entity = node.entity();
    match entity.get("id") {
        Some(holon_api::Value::String(s)) => Some(s.clone()),
        Some(holon_api::Value::Integer(i)) => Some(i.to_string()),
        _ => None,
    }
}

fn build_navigator(
    widget: &str,
    items: &[Arc<ReactiveViewModel>],
) -> Option<Box<dyn CollectionNavigator>> {
    let ids: Vec<String> = items
        .iter()
        .filter_map(|item| resolve_entity_id(item))
        .collect();
    if ids.is_empty() {
        return None;
    }

    match widget {
        "tree" | "outline" => {
            let mut dfs_order = Vec::new();
            let mut parent_map = HashMap::new();
            collect_tree_structure(items, &mut dfs_order, &mut parent_map);
            if dfs_order.is_empty() {
                return None;
            }
            Some(Box::new(TreeNavigator::from_dfs_and_parents(
                dfs_order, parent_map,
            )))
        }
        _ => Some(Box::new(ListNavigator::new(ids))),
    }
}

fn collect_tree_structure(
    items: &[Arc<ReactiveViewModel>],
    dfs_order: &mut Vec<String>,
    parent_map: &mut HashMap<String, String>,
) {
    let mut stack: Vec<(usize, String)> = Vec::new();

    for item in items {
        let (depth, content) = match item.widget_name().as_deref() {
            Some("tree_item") => {
                let d = item.prop_f64("depth").unwrap_or(0.0) as usize;
                (d, item.children.first().map(|c| c.as_ref()))
            }
            _ => {
                if let Some(id) = resolve_entity_id(item) {
                    dfs_order.push(id);
                }
                continue;
            }
        };

        let id = match content.and_then(resolve_entity_id) {
            Some(id) => id,
            None => continue,
        };

        while stack.last().map_or(false, |(d, _)| *d >= depth) {
            stack.pop();
        }

        if let Some((_, parent)) = stack.last() {
            parent_map.insert(id.clone(), parent.clone());
        }

        dfs_order.push(id.clone());
        stack.push((depth, id));
    }
}

// ── Diagnostic ─────────────────────────────────────────────────────────

fn describe_tree(node: &ReactiveViewModel, depth: usize) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let indent = "  ".repeat(depth);
    let widget = node.widget_name().unwrap_or_else(|| "?".to_string());
    let eid = resolve_entity_id(node).unwrap_or_else(|| "-".to_string());
    let children = collect_children(node);
    let nav = if is_collection_widget(Some(&widget)) {
        " [NAV]"
    } else {
        ""
    };
    writeln!(
        out,
        "{indent}{widget} id={eid} children={}{nav}",
        children.len()
    )
    .ok();

    if depth < 4 {
        for child in &children {
            out.push_str(&describe_tree(child, depth + 1));
        }
    } else if !children.is_empty() {
        writeln!(out, "{indent}  ... ({} children)", children.len()).ok();
    }
    out
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::navigation::{Boundary, CursorHint, CursorPlacement, NavDirection};
    use holon_api::{EntityUri, Value};
    use std::collections::HashMap as StdHashMap;

    fn make_row(id: &str) -> ReactiveViewModel {
        let data = Arc::new(StdHashMap::from([("id".into(), Value::String(id.into()))]));
        ReactiveViewModel::from_widget("table_row", StdHashMap::new()).with_entity(data)
    }

    fn column(children: Vec<ReactiveViewModel>) -> ReactiveViewModel {
        ReactiveViewModel::layout("column", children)
    }

    fn row(children: Vec<ReactiveViewModel>) -> ReactiveViewModel {
        ReactiveViewModel::layout("row", children)
    }

    fn nested_live_block(block_id: &str) -> ReactiveViewModel {
        ReactiveViewModel::live_block(EntityUri::from_raw(block_id))
    }

    fn list(items: Vec<ReactiveViewModel>) -> ReactiveViewModel {
        ReactiveViewModel::static_collection("list", items, 0.0)
    }

    #[test]
    fn build_and_navigate() {
        let tree = Arc::new(list(vec![make_row("a"), make_row("b"), make_row("c")]));
        let fp = build_focus_path(&tree, "a").expect("should find 'a'");

        let input = WidgetInput::Navigate {
            direction: NavDirection::Down,
            hint: CursorHint {
                column: 5,
                boundary: Boundary::Bottom,
            },
        };

        match fp.bubble_input("a", &input) {
            Some(InputAction::Focus {
                block_id,
                placement,
            }) => {
                assert_eq!(block_id, "b");
                assert_eq!(placement, CursorPlacement::FirstLine { column: 5 });
            }
            other => panic!("expected Focus, got {other:?}"),
        }

        // Last item: navigation returns None
        let fp_c = build_focus_path(&tree, "c").expect("should find 'c'");
        assert!(fp_c.bubble_input("c", &input).is_none());
    }

    #[test]
    fn bubble_keychord() {
        use holon_api::render_types::{OperationDescriptor, OperationWiring, Trigger};

        let mut vm = make_row("entity-1");
        vm.operations.push(OperationWiring {
            modified_param: "id".into(),
            descriptor: OperationDescriptor {
                name: "cycle_task_state".into(),
                entity_name: "block".into(),
                trigger: Some(Trigger::KeyChord {
                    chord: KeyChord::new(&[crate::input::Key::Cmd, crate::input::Key::Enter]),
                }),
                ..Default::default()
            },
        });

        let tree = Arc::new(column(vec![vm]));
        let fp = build_focus_path(&tree, "entity-1").expect("should find entity");

        let input = WidgetInput::chord(&[crate::input::Key::Cmd, crate::input::Key::Enter]);
        match fp.bubble_input("entity-1", &input) {
            Some(InputAction::ExecuteOperation {
                entity_name,
                operation,
                entity_id,
            }) => {
                assert_eq!(entity_name, "block");
                assert_eq!(operation.name, "cycle_task_state");
                assert_eq!(entity_id, "entity-1");
            }
            other => panic!("expected ExecuteOperation, got {other:?}"),
        }

        // Unmatched chord returns None
        let unmatched = WidgetInput::chord(&[crate::input::Key::Cmd, crate::input::Key::Char('z')]);
        assert!(fp.bubble_input("entity-1", &unmatched).is_none());
    }

    #[test]
    fn nonexistent_entity_returns_none() {
        let tree = Arc::new(list(vec![make_row("a")]));
        assert!(build_focus_path(&tree, "nonexistent").is_none());
    }

    #[test]
    fn cross_block_focus_path() {
        let root_tree = Arc::new(column(vec![nested_live_block("block:inner")]));

        let inner_content = Arc::new(list(vec![make_row("inner-1"), make_row("inner-2")]));

        let mut block_contents = HashMap::new();
        block_contents.insert("block:inner".to_string(), inner_content);

        let fp = build_focus_path_cross_block(&root_tree, &block_contents, "inner-1")
            .expect("should find inner-1 across block boundary");

        let input = WidgetInput::Navigate {
            direction: NavDirection::Down,
            hint: CursorHint {
                column: 0,
                boundary: Boundary::Bottom,
            },
        };

        match fp.bubble_input("inner-1", &input) {
            Some(InputAction::Focus { block_id, .. }) => {
                assert_eq!(block_id, "inner-2");
            }
            other => panic!("expected Focus to inner-2, got {other:?}"),
        }
    }

    #[test]
    fn collect_all_entity_ids_traverses_tree() {
        let tree = column(vec![make_row("a"), row(vec![make_row("b"), make_row("c")])]);
        let ids = collect_all_entity_ids(&tree);
        assert!(ids.contains(&"a".to_string()));
        assert!(ids.contains(&"b".to_string()));
        assert!(ids.contains(&"c".to_string()));
    }

    #[test]
    fn input_router_caches_and_navigates() {
        let tree = Arc::new(list(vec![make_row("a"), make_row("b"), make_row("c")]));
        let router = InputRouter::new();
        router.set_root(tree);

        let input = WidgetInput::Navigate {
            direction: NavDirection::Down,
            hint: CursorHint {
                column: 0,
                boundary: Boundary::Bottom,
            },
        };

        // First call builds the path
        match router.bubble_input("a", &input) {
            Some(InputAction::Focus { block_id, .. }) => assert_eq!(block_id, "b"),
            other => panic!("expected Focus to b, got {other:?}"),
        }

        // After navigation resolved to "b", calling again with "b" should
        // reuse the prefix and navigate to "c"
        match router.bubble_input("b", &input) {
            Some(InputAction::Focus { block_id, .. }) => assert_eq!(block_id, "c"),
            other => panic!("expected Focus to c, got {other:?}"),
        }

        // Last element: None
        assert!(router.bubble_input("c", &input).is_none());
    }

    #[test]
    fn find_click_intent_oneshot_returns_bound_action() {
        use holon_api::render_types::{OperationDescriptor, OperationWiring, Trigger};
        use holon_api::EntityName;

        // Sidebar-shaped tree: list → selectable(row(text(...))) per item.
        // Each selectable carries a click-bound `navigation.focus` op.
        let mut sidebar_item = make_row("doc:foo");
        sidebar_item.operations.push(OperationWiring {
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: EntityName::new("navigation"),
                name: "focus".into(),
                trigger: Some(Trigger::Click),
                bound_params: StdHashMap::from([
                    ("region".into(), Value::String("main".into())),
                    ("block_id".into(), Value::String("doc:foo".into())),
                ]),
                ..Default::default()
            },
        });

        let tree = list(vec![sidebar_item, make_row("doc:bar")]);

        // Click on the item with a bound action → returns the navigation.focus intent.
        let intent = find_click_intent_oneshot(&tree, "doc:foo")
            .expect("doc:foo should yield a click intent");
        assert_eq!(intent.entity_name.as_str(), "navigation");
        assert_eq!(intent.op_name, "focus");
        assert_eq!(
            intent.params.get("block_id").and_then(|v| v.as_string()),
            Some("doc:foo")
        );

        // Click on the item without a bound action → None (driver falls back).
        assert!(find_click_intent_oneshot(&tree, "doc:bar").is_none());

        // Click on a non-existent entity → None.
        assert!(find_click_intent_oneshot(&tree, "doc:nope").is_none());
    }
}
