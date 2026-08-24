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

use holon_api::EntityUri;
use holon_api::render_types::OperationWiring;

use crate::input::InputAction;
use crate::input::KeyChord;
use crate::input::WidgetInput;
use crate::navigation::CollectionNavigator;
use crate::navigation::ListNavigator;
use crate::navigation::TreeNavigator;
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
    pub fn bubble_input(&self, entity_id: &EntityUri, input: &WidgetInput) -> Option<InputAction> {
        for entry in self.path.iter().rev() {
            if let Some(action) = try_handle(entry, entity_id, input) {
                return Some(action);
            }
        }
        None
    }

    /// The entity IDs along the path (root to focused).
    pub fn entity_ids(&self) -> Vec<Option<EntityUri>> {
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
pub fn build_focus_path(root: &Arc<ReactiveViewModel>, entity_id: &EntityUri) -> Option<FocusPath> {
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
    entity_id: &EntityUri,
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
    entity_id: &EntityUri,
) -> Option<crate::operations::OperationIntent> {
    fn walk(
        node: &ReactiveViewModel,
        entity_id: &EntityUri,
    ) -> Option<crate::operations::OperationIntent> {
        if resolve_entity_id(node).as_ref() == Some(entity_id) {
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

/// Static-snapshot variant of `find_click_intent_oneshot` for `ViewModel`
/// trees.
///
/// Used when the live reactive tree's `live_block` slots haven't been filled
/// yet (a common headless-test situation where no consumer drains per-block
/// streams into slots). The caller obtains a fully-resolved `ViewModel` via
/// `BuilderServices::snapshot_resolved`, which recursively interprets every
/// nested block, then walks it here. The `OperationWiring` info is identical
/// across both representations, so the resulting `OperationIntent` matches
/// what GPUI would dispatch on a real click.
///
/// `modifiers` selects WHICH click wiring is returned: `ClickModifiers::none()`
/// is the primary click, `ClickModifiers::shift()` the `shift_action:` wiring,
/// and so on. A node binding only a modifier action yields `None` for a
/// primary click (and vice versa) — the same discrimination the GPUI
/// `selectable` handler performs on its `HashMap<ClickModifiers, _>`.
pub fn find_click_intent_in_view_model(
    root: &crate::view_model::ViewModel,
    entity_id: &EntityUri,
    modifiers: holon_api::ClickModifiers,
) -> Option<crate::operations::OperationIntent> {
    fn walk(
        node: &crate::view_model::ViewModel,
        entity_id: &EntityUri,
        modifiers: holon_api::ClickModifiers,
    ) -> Option<crate::operations::OperationIntent> {
        if node.entity_id().as_ref() == Some(entity_id) {
            if let Some(intent) = crate::operations::click_intent_for(&node.operations, modifiers) {
                return Some(intent);
            }
        }
        for child in node.children() {
            if let Some(intent) = walk(child, entity_id, modifiers) {
                return Some(intent);
            }
        }
        None
    }
    walk(root, entity_id, modifiers)
}

/// Region-scoped variant: only walk the subtree rooted at the clicked region's
/// panel. Production GPUI's click handler runs on a specific element in a
/// specific panel; the same entity_id can appear in multiple regions (e.g.
/// `block:journals` shows up both in the LeftSidebar list AND in the Main
/// panel when focused), and each region's wrapper may bind a different action.
/// The unscoped variant returns the FIRST match in DFS order, which crosses
/// region boundaries and breaks production parity.
///
/// Returns `None` for unknown regions or when the entity isn't reachable from
/// the panel's subtree (matching production: a click on a region the user
/// can't reach does nothing).
/// Resolve a `region` name to its panel subtree within the layout `root`.
fn find_region_panel<'a>(
    root: &'a crate::view_model::ViewModel,
    region: &str,
) -> Option<&'a crate::view_model::ViewModel> {
    let panel_id = match region {
        "left_sidebar" => "block:default-left-sidebar",
        "main" => "block:default-main-panel",
        "right_sidebar" => "block:default-right-sidebar",
        _ => return None,
    };
    let panel_id =
        EntityUri::parse(panel_id).expect("static panel-key literals are valid EntityUris");

    fn find_panel<'a>(
        node: &'a crate::view_model::ViewModel,
        panel_id: &EntityUri,
    ) -> Option<&'a crate::view_model::ViewModel> {
        if node.entity_id().as_ref() == Some(panel_id) {
            return Some(node);
        }
        for child in node.children() {
            if let Some(found) = find_panel(child, panel_id) {
                return Some(found);
            }
        }
        None
    }

    find_panel(root, &panel_id)
}

/// True if `region`'s panel is rendered in `root` at all.
///
/// Distinguishes "the layout has not been rendered yet" from "the panel is
/// there but empty" — the two states every region-scoped lookup collapses into
/// a bare `None`. A frontend that has not yet resolved its layout answers
/// `false` for every region.
pub fn region_panel_present(root: &crate::view_model::ViewModel, region: &str) -> bool {
    find_region_panel(root, region).is_some()
}

pub fn find_click_intent_in_region(
    root: &crate::view_model::ViewModel,
    entity_id: &EntityUri,
    region: &str,
    modifiers: holon_api::ClickModifiers,
) -> Option<crate::operations::OperationIntent> {
    let panel = find_region_panel(root, region)?;
    find_click_intent_in_view_model(panel, entity_id, modifiers)
}

/// Name why [`find_click_intent_in_region`] resolved nothing for `entity_id`.
///
/// The bare `None` conflates three structurally different states — no region
/// panel, the entity absent from the panel, and the entity present but binding
/// no click wiring for these `modifiers`. Only the second is a readiness race
/// worth polling; a caller that guesses sends the investigation the wrong way.
pub fn click_intent_miss_reason(
    root: &crate::view_model::ViewModel,
    entity_id: &EntityUri,
    region: &str,
    modifiers: holon_api::ClickModifiers,
) -> String {
    fn collect_matches<'a>(
        node: &'a crate::view_model::ViewModel,
        entity_id: &EntityUri,
        out: &mut Vec<&'a crate::view_model::ViewModel>,
    ) {
        if node.entity_id().as_ref() == Some(entity_id) {
            out.push(node);
        }
        for child in node.children() {
            collect_matches(child, entity_id, out);
        }
    }

    fn collect_ids(node: &crate::view_model::ViewModel, out: &mut Vec<String>) {
        if let Some(id) = node.entity_id() {
            out.push(id.to_string());
        }
        for child in node.children() {
            collect_ids(child, out);
        }
    }

    let Some(panel) = find_region_panel(root, region) else {
        return format!("region {region} renders no panel in the resolved tree at all");
    };

    let mut matched = Vec::new();
    collect_matches(panel, entity_id, &mut matched);
    if matched.is_empty() {
        let mut ids = Vec::new();
        collect_ids(panel, &mut ids);
        ids.sort();
        ids.dedup();
        return format!(
            "{entity_id} renders NO node in region {region} — the panel is not showing this \
             entity at all. It renders {} distinct entities: [{}]",
            ids.len(),
            ids.join(", ")
        );
    }

    let bound: Vec<String> = matched
        .iter()
        .flat_map(|n| n.operations.iter())
        .map(|ow| {
            format!(
                "{}.{} @ {:?}",
                ow.descriptor.entity_name,
                ow.descriptor.name,
                ow.descriptor.click_modifiers()
            )
        })
        .collect();
    format!(
        "{entity_id} DOES render {} node(s) in region {region}, but none binds a click wiring for \
         modifiers {modifiers:?} — the row is there, the action is not. Bound operations: [{}]",
        matched.len(),
        bound.join(", ")
    )
}

/// Resolve the intent a click on `entity_id`'s `state_toggle` glyph dispatches,
/// within `region`'s panel. `state_toggle` cycling is NOT a bound click-INTENT:
/// `cycle_task_state` is key-chord-bound (Cmd+Enter), so `find_click_intent_*`
/// deliberately ignores it. Instead the GPUI `state_toggle` widget hardcodes
/// its `on_mouse_down` to compute the NEXT cycle value and dispatch a
/// `set_field` write (see
/// `frontends/gpui/src/render/builders/state_toggle.rs`). This mirrors
/// that exact behaviour so the headless driver's `click_entity` advances a
/// visible task row's state the same way the windowed geometry click does —
/// closing the headless/windowed parity gap that made `state_toggle` clicks a
/// no-op headless.
pub fn state_toggle_cycle_intent(
    root: &crate::view_model::ViewModel,
    entity_id: &EntityUri,
    region: &str,
) -> Option<crate::operations::OperationIntent> {
    use crate::view_model::ViewKind;

    fn walk(
        node: &crate::view_model::ViewModel,
        entity_id: &EntityUri,
    ) -> Option<crate::operations::OperationIntent> {
        if let ViewKind::StateToggle {
            field,
            current,
            states,
            ..
        } = &node.kind
        {
            if node.entity_id().as_ref() == Some(entity_id) {
                return crate::operations::state_toggle_intent(
                    field,
                    current,
                    states,
                    &node.operations,
                    node.entity_name().as_ref(),
                    node.row_id().as_deref(),
                );
            }
        }
        for child in node.children() {
            if let Some(intent) = walk(child, entity_id) {
                return Some(intent);
            }
        }
        None
    }

    let panel = find_region_panel(root, region)?;
    walk(panel, entity_id)
}

/// The task keyword `entity_id`'s rendered `state_toggle` shows, or `None` when
/// the row renders no toggle node at all.
///
/// An EMPTY string is the plain-row rendering, not an absent one: the widget
/// collapses to a zero-width spacer and paints no glyph when `current` is empty
/// (see `frontends/gpui/src/render/builders/state_toggle.rs`). So "the task
/// affordance is on screen" is `Some(non-empty)`, and a test that only asked
/// whether the node exists would pass on every plain block.
pub fn state_toggle_current(
    root: &crate::view_model::ViewModel,
    entity_id: &EntityUri,
    region: &str,
) -> Option<String> {
    use crate::view_model::ViewKind;

    fn walk(node: &crate::view_model::ViewModel, entity_id: &EntityUri) -> Option<String> {
        if let ViewKind::StateToggle { current, .. } = &node.kind {
            if node.entity_id().as_ref() == Some(entity_id) {
                return Some(current.clone());
            }
        }
        node.children().iter().find_map(|c| walk(c, entity_id))
    }

    let panel = find_region_panel(root, region)?;
    walk(panel, entity_id)
}

/// Name why [`state_toggle_cycle_intent`] resolved nothing for `entity_id`.
///
/// That function returns a bare `None` for four structurally different states —
/// no region panel, the entity absent from the region, the entity present but
/// rendering no `state_toggle`, and the glyph present but binding no
/// `set_field` op to dispatch. Only the third is "not a task row"; a driver
/// that guesses sends every investigation down the wrong path.
pub fn state_toggle_miss_reason(
    root: &crate::view_model::ViewModel,
    entity_id: &EntityUri,
    region: &str,
) -> String {
    use crate::view_model::ViewKind;

    fn collect_matches<'a>(
        node: &'a crate::view_model::ViewModel,
        entity_id: &EntityUri,
        out: &mut Vec<&'a crate::view_model::ViewModel>,
    ) {
        if node.entity_id().as_ref() == Some(entity_id) {
            out.push(node);
        }
        for child in node.children() {
            collect_matches(child, entity_id, out);
        }
    }

    fn collect_ids(node: &crate::view_model::ViewModel, out: &mut Vec<String>) {
        if let Some(id) = node.entity_id() {
            out.push(id.to_string());
        }
        for child in node.children() {
            collect_ids(child, out);
        }
    }

    let Some(panel) = find_region_panel(root, region) else {
        return format!("region {region} renders no panel in the resolved tree at all");
    };

    let mut matched = Vec::new();
    collect_matches(panel, entity_id, &mut matched);
    if matched.is_empty() {
        let mut ids = Vec::new();
        collect_ids(panel, &mut ids);
        ids.sort();
        ids.dedup();
        return format!(
            "{entity_id} renders NO node in region {region} — the panel is not showing this \
             block. It renders {} distinct entities: [{}]",
            ids.len(),
            ids.join(", ")
        );
    }

    let kinds: Vec<&str> = matched
        .iter()
        .filter_map(|n| n.widget_name())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let Some(toggle) = matched
        .iter()
        .find(|n| matches!(n.kind, ViewKind::StateToggle { .. }))
    else {
        return format!(
            "{entity_id} renders {} node(s) in region {region} but NONE is a state_toggle — the \
             row is there, its glyph is not. Rendered as: [{}]",
            matched.len(),
            kinds.join(", ")
        );
    };

    let ViewKind::StateToggle { field, .. } = &toggle.kind else {
        unreachable!("matched on StateToggle above")
    };
    let ops: Vec<&str> = toggle
        .operations
        .iter()
        .map(|ow| ow.descriptor.name.as_str())
        .collect();
    format!(
        "{entity_id} DOES render a state_toggle on field `{field}` in region {region}, but no \
         set_field op is wired to it — clicking the glyph would dispatch nothing. Bound ops: [{}]",
        ops.join(", ")
    )
}

/// True if `entity_id` is rendered anywhere within `region`'s panel subtree.
///
/// Mirrors `find_click_intent_in_region`'s traversal (same panel scope, same
/// `children()` DFS) but only asks "is it there?", not "does it bind a click".
/// `click_entity` uses this to tell apart two states its intent-poll otherwise
/// conflates: the entity isn't rendered *yet* (a genuine readiness race worth
/// polling) versus it's rendered but carries no bound click-intent (e.g. an
/// `editable_text` block in Main, where click = cursor placement = focus) —
/// where polling can never surface an intent and is pure wasted wall time.
pub fn region_contains_entity(
    root: &crate::view_model::ViewModel,
    entity_id: &EntityUri,
    region: &str,
) -> bool {
    fn contains(node: &crate::view_model::ViewModel, entity_id: &EntityUri) -> bool {
        if node.entity_id().as_ref() == Some(entity_id) {
            return true;
        }
        node.children().iter().any(|c| contains(c, entity_id))
    }
    find_region_panel(root, region).is_some_and(|panel| contains(panel, entity_id))
}

/// Build a `FocusPath` across block boundaries for the headless path.
///
/// `live_block` nodes in the root tree have empty slots (content is populated
/// independently per block). `block_contents` maps block_id → latest
/// `ReactiveViewModel` for that block's content.
pub fn build_focus_path_cross_block(
    root_content: &Arc<ReactiveViewModel>,
    block_contents: &HashMap<EntityUri, Arc<ReactiveViewModel>>,
    entity_id: &EntityUri,
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
    entity_id: &EntityUri,
    input: &WidgetInput,
) -> Option<InputAction> {
    match dfs_and_bubble(root, entity_id, input) {
        DfsResult::Handled(action) => Some(*action),
        _ => None,
    }
}

enum DfsResult {
    /// Entity not found in this subtree.
    NotFound,
    /// Entity found but no ancestor handled the input.
    Found,
    /// Entity found and input was handled.
    Handled(Box<InputAction>),
}

/// Recursive DFS that bubbles on the way back up.
///
/// When the target entity is found, returns `Found`. Each ancestor frame
/// then tries to handle the input. The first match returns `Handled`.
fn dfs_and_bubble(
    node: &ReactiveViewModel,
    entity_id: &EntityUri,
    input: &WidgetInput,
) -> DfsResult {
    if resolve_entity_id(node).as_ref() == Some(entity_id) {
        if let Some(action) = try_handle_node(node, entity_id, input) {
            return DfsResult::Handled(Box::new(action));
        }
        return DfsResult::Found;
    }

    for child in collect_children(node) {
        match dfs_and_bubble(&child, entity_id, input) {
            DfsResult::Handled(action) => return DfsResult::Handled(action),
            DfsResult::Found => {
                if let Some(action) = try_handle_node(node, entity_id, input) {
                    return DfsResult::Handled(Box::new(action));
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
    origin_id: &EntityUri,
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
                entity_id: origin_id.clone(),
            })
        }
    }
}

/// Collect all entity IDs reachable from `root` via DFS.
/// Standalone utility replacing `IncrementalShadowIndex::entity_ids()`.
pub fn collect_all_entity_ids(root: &ReactiveViewModel) -> Vec<EntityUri> {
    let mut ids = Vec::new();
    collect_ids_dfs(root, &mut ids);
    ids
}

fn collect_ids_dfs(node: &ReactiveViewModel, ids: &mut Vec<EntityUri>) {
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
pub type LiveBlockResolver =
    Arc<dyn Fn(&EntityUri) -> Option<Arc<ReactiveViewModel>> + Send + Sync>;

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
    entity_id: EntityUri,
    focus_path: FocusPath,
}

impl Default for InputRouter {
    fn default() -> Self {
        Self::new()
    }
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
    pub fn bubble_input(&self, entity_id: &EntityUri, input: &WidgetInput) -> Option<InputAction> {
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

    /// The root tree a frontend last published, for callers that must hand a
    /// real `root_tree` to a `UserDriver` verb rather than fabricate one.
    pub fn root_tree(&self) -> Option<Arc<ReactiveViewModel>> {
        self.root.read().unwrap().clone()
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
                    let eid = resolve_entity_id(&entry.node)
                        .map(|u| u.to_string())
                        .unwrap_or_else(|| "-".to_string());
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

    fn ensure_focus_path(&self, entity_id: &EntityUri) {
        {
            let guard = self.cached.read().unwrap();
            if let Some(ref cached) = *guard {
                if &cached.entity_id == entity_id {
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
                    entity_id: entity_id.clone(),
                    focus_path: fp,
                });
            }
        }
    }

    /// After navigation returns `Focus { block_id }`, try to reuse the
    /// common prefix of the cached path (up to the collection parent) and
    /// only DFS from there to find the new target. Falls back to full
    /// rebuild if the optimization doesn't apply.
    fn update_cache_for_navigation(&self, new_entity_id: &EntityUri) {
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
                        entity_id: new_entity_id.clone(),
                        focus_path: FocusPath { path: new_path },
                    });
                    return;
                }
            }
        }

        // ALLOW(fallback): disclosed — when the cached common-prefix optimization
        // can't apply, rebuild the focus path with a full DFS from root.
        let fp = match resolver {
            Some(r) => build_focus_path_with_resolver(root, new_entity_id, r.as_ref()),
            None => build_focus_path(root, new_entity_id),
        };
        if let Some(fp) = fp {
            *cache_guard = Some(CachedFocusPath {
                entity_id: new_entity_id.clone(),
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
    entity_id: &EntityUri,
    stack: &mut Vec<Arc<ReactiveViewModel>>,
) -> bool {
    stack.push(node.clone());

    if resolve_entity_id(node).as_ref() == Some(entity_id) {
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
    block_contents: &HashMap<EntityUri, Arc<ReactiveViewModel>>,
    entity_id: &EntityUri,
    stack: &mut Vec<Arc<ReactiveViewModel>>,
) -> bool {
    stack.push(node.clone());

    if resolve_entity_id(node).as_ref() == Some(entity_id) {
        return true;
    }

    // If this is a live_block, look up the block's content.
    if node.widget_name().as_deref() == Some("live_block") {
        if let Some(block_id) = node.prop_str("block_id") {
            let block_id = EntityUri::parse(&block_id)
                .expect("live_block props[\"block_id\"] must be a schemed EntityUri");
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
    entity_id: &EntityUri,
    resolver: &(dyn Fn(&EntityUri) -> Option<Arc<ReactiveViewModel>> + Send + Sync),
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
    entity_id: &EntityUri,
    resolver: &(dyn Fn(&EntityUri) -> Option<Arc<ReactiveViewModel>> + Send + Sync),
    stack: &mut Vec<Arc<ReactiveViewModel>>,
) -> bool {
    stack.push(node.clone());

    if resolve_entity_id(node).as_ref() == Some(entity_id) {
        return true;
    }

    if node.widget_name().as_deref() == Some("live_block") {
        if let Some(block_id) = node.prop_str("block_id") {
            let block_id = EntityUri::parse(&block_id)
                .expect("live_block props[\"block_id\"] must be a schemed EntityUri");
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

fn try_handle(
    entry: &FocusPathEntry,
    origin_id: &EntityUri,
    input: &WidgetInput,
) -> Option<InputAction> {
    match input {
        WidgetInput::Navigate { direction, hint } => {
            try_navigate(entry, origin_id, *direction, hint)
        }
        WidgetInput::KeyChord { keys } => try_keychord(entry, origin_id, keys),
    }
}

fn try_navigate(
    entry: &FocusPathEntry,
    origin_id: &EntityUri,
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
    origin_id: &EntityUri,
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
        entity_id: origin_id.clone(),
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

/// Resolve the typed `EntityUri` for a node. Returns the explicit ID for
/// nodes that have one, otherwise falls back to `entity().get("id")` —
/// parsed once at this boundary via the centralized `entity_uri_from_id_str`
/// seam, so the whole routing layer compares `EntityUri`s and a
/// bare-vs-schemed mismatch is impossible past this point.
pub fn resolve_entity_id(node: &ReactiveViewModel) -> Option<EntityUri> {
    if let Some(id) = node.entity_id() {
        return Some(id);
    }
    let entity = node.entity();
    match entity.get("id") {
        Some(holon_api::Value::String(s)) => Some(holon_api::entity_uri_from_id_str(s)),
        Some(holon_api::Value::Integer(i)) => {
            Some(holon_api::entity_uri_from_id_str(&i.to_string()))
        }
        _ => None,
    }
}

fn build_navigator(
    widget: &str,
    items: &[Arc<ReactiveViewModel>],
) -> Option<Box<dyn CollectionNavigator>> {
    let ids: Vec<EntityUri> = items
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
    dfs_order: &mut Vec<EntityUri>,
    parent_map: &mut HashMap<EntityUri, EntityUri>,
) {
    let mut stack: Vec<(usize, EntityUri)> = Vec::new();

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

        while stack.last().is_some_and(|(d, _)| *d >= depth) {
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
    let eid = resolve_entity_id(node)
        .map(|u| u.to_string())
        .unwrap_or_else(|| "-".to_string());
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
    use std::collections::HashMap as StdHashMap;

    use holon_api::EntityUri;
    use holon_api::Value;

    use super::*;
    use crate::navigation::Boundary;
    use crate::navigation::CursorHint;
    use crate::navigation::CursorPlacement;
    use crate::navigation::NavDirection;

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
        ReactiveViewModel::live_block(
            EntityUri::parse(block_id).expect("test fixture ids are schemed"),
        )
    }

    fn list(items: Vec<ReactiveViewModel>) -> ReactiveViewModel {
        ReactiveViewModel::static_collection("list", items, 0.0, false, Default::default())
    }

    /// Test-helper mirroring the typed-id boundary: bare → `block:`,
    /// schemed → parsed as-is. Same canonicalisation `resolve_entity_id`
    /// applies to fixture row ids.
    fn uri(s: &str) -> EntityUri {
        holon_api::entity_uri_from_id_str(s)
    }

    #[test]
    fn build_and_navigate() {
        let tree = Arc::new(list(vec![make_row("a"), make_row("b"), make_row("c")]));
        let fp = build_focus_path(&tree, &uri("a")).expect("should find 'a'");

        let input = WidgetInput::Navigate {
            direction: NavDirection::Down,
            hint: CursorHint {
                column: 5,
                boundary: Boundary::Bottom,
            },
        };

        match fp.bubble_input(&uri("a"), &input) {
            Some(InputAction::Focus {
                block_id,
                placement,
            }) => {
                assert_eq!(block_id, uri("b"));
                assert_eq!(placement, CursorPlacement::FirstLine { column: 5 });
            }
            other => panic!("expected Focus, got {other:?}"),
        }

        // Last item: navigation returns None
        let fp_c = build_focus_path(&tree, &uri("c")).expect("should find 'c'");
        assert!(fp_c.bubble_input(&uri("c"), &input).is_none());
    }

    #[test]
    fn bubble_keychord() {
        use holon_api::render_types::OperationDescriptor;
        use holon_api::render_types::OperationWiring;
        use holon_api::render_types::Trigger;

        let mut vm = make_row("entity-1");
        vm.operations.push(OperationWiring {
            modified_param: "id".into(),
            descriptor: OperationDescriptor {
                name: "cycle_task_state".into(),
                entity_name: "block".into(),
                trigger: Some(Trigger::KeyChord {
                    chord: KeyChord::new(&[crate::input::Key::Cmd, crate::input::Key::Enter]),
                }),
                entity_short_name: String::new(),
                id_column: "id".to_string(),
                display_name: String::new(),
                description: String::new(),
                required_params: vec![],
                affected_fields: vec![],
                param_mappings: vec![],
                bound_params: Default::default(),
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                menu_exposure: holon_api::MenuExposure::Listed {
                    surfaces: holon_api::SurfaceSet {
                        slash_menu: true,
                        action_bar: false,
                    },
                },
                marking_delta: holon_api::marking::MarkingDelta::Undeclared,
                guard: holon_api::pattern::OpGuard::None,
                arcs: holon_api::arcs::TransitionArcs::Undeclared,
            },
        });

        let tree = Arc::new(column(vec![vm]));
        let fp = build_focus_path(&tree, &uri("entity-1")).expect("should find entity");

        let input = WidgetInput::chord(&[crate::input::Key::Cmd, crate::input::Key::Enter]);
        match fp.bubble_input(&uri("entity-1"), &input) {
            Some(InputAction::ExecuteOperation {
                entity_name,
                operation,
                entity_id,
            }) => {
                assert_eq!(entity_name, "block");
                assert_eq!(operation.name, "cycle_task_state");
                assert_eq!(entity_id, uri("entity-1"));
            }
            other => panic!("expected ExecuteOperation, got {other:?}"),
        }

        // Unmatched chord returns None
        let unmatched = WidgetInput::chord(&[crate::input::Key::Cmd, crate::input::Key::Char('z')]);
        assert!(fp.bubble_input(&uri("entity-1"), &unmatched).is_none());
    }

    /// Cmd+Enter on a focused-but-not-editing block must resolve to
    /// `cycle_task_state` via the key chord routing. This tests the
    /// headless routing path — the GPUI frontend must also wire
    /// `capture_action(Enter)` in `render_entity_view` (dogfood Risk 2).
    #[test]
    fn cmd_enter_routes_to_cycle_task_state_non_editing() {
        use holon_api::render_types::OperationDescriptor;
        use holon_api::render_types::OperationWiring;
        use holon_api::render_types::Trigger;

        // Simulate a block rendered through the default (non-editing)
        // profile, with operations joined from key_bindings.
        let mut vm = make_row("entity-2");
        vm.operations = vec![
            // The state_toggle builder hardcodes set_field on click;
            // cycle_task_state is key-chord-only (Cmd+Enter).
            OperationWiring {
                modified_param: "id".into(),
                descriptor: OperationDescriptor {
                    name: "set_field".into(),
                    entity_name: "block".into(),
                    trigger: None,
                    entity_short_name: String::new(),
                    id_column: "id".to_string(),
                    display_name: String::new(),
                    description: String::new(),
                    required_params: vec![],
                    affected_fields: vec![],
                    param_mappings: vec![],
                    bound_params: Default::default(),
                    target_scope: holon_api::TargetScope::Block,
                    boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                    menu_exposure: holon_api::MenuExposure::NotListed {
                        surface: holon_api::NonMenuSurface::Test,
                    },
                    marking_delta: holon_api::marking::MarkingDelta::Undeclared,
                    guard: holon_api::pattern::OpGuard::None,
                    arcs: holon_api::arcs::TransitionArcs::Undeclared,
                },
            },
            OperationWiring {
                modified_param: "id".into(),
                descriptor: OperationDescriptor {
                    name: "cycle_task_state".into(),
                    entity_name: "block".into(),
                    trigger: Some(Trigger::KeyChord {
                        chord: KeyChord::new(&[crate::input::Key::Cmd, crate::input::Key::Enter]),
                    }),
                    entity_short_name: String::new(),
                    id_column: "id".to_string(),
                    display_name: String::new(),
                    description: String::new(),
                    required_params: vec![],
                    affected_fields: vec![],
                    param_mappings: vec![],
                    bound_params: Default::default(),
                    target_scope: holon_api::TargetScope::Block,
                    boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                    menu_exposure: holon_api::MenuExposure::Listed {
                        surfaces: holon_api::SurfaceSet {
                            slash_menu: true,
                            action_bar: false,
                        },
                    },
                    marking_delta: holon_api::marking::MarkingDelta::Undeclared,
                    guard: holon_api::pattern::OpGuard::None,
                    arcs: holon_api::arcs::TransitionArcs::Undeclared,
                },
            },
        ];

        let tree = Arc::new(column(vec![vm]));
        let fp = build_focus_path(&tree, &uri("entity-2")).expect("entity-2 not found");

        let input = WidgetInput::chord(&[crate::input::Key::Cmd, crate::input::Key::Enter]);
        match fp.bubble_input(&uri("entity-2"), &input) {
            Some(InputAction::ExecuteOperation {
                entity_name,
                operation,
                ..
            }) => {
                assert_eq!(entity_name, "block");
                assert_eq!(operation.name, "cycle_task_state");
            }
            other => panic!("Cmd+Enter should resolve to cycle_task_state, got {other:?}"),
        }

        // click on state_toggle (no key chord) must NOT resolve to
        // cycle_task_state — the click intent is separate.
        assert!(find_click_intent_oneshot(&tree, &uri("entity-2")).is_none());
    }

    #[test]
    fn nonexistent_entity_returns_none() {
        let tree = Arc::new(list(vec![make_row("a")]));
        assert!(build_focus_path(&tree, &uri("nonexistent")).is_none());
    }

    #[test]
    fn cross_block_focus_path() {
        let root_tree = Arc::new(column(vec![nested_live_block("block:inner")]));

        let inner_content = Arc::new(list(vec![make_row("inner-1"), make_row("inner-2")]));

        let mut block_contents = HashMap::new();
        block_contents.insert(uri("block:inner"), inner_content);

        let fp = build_focus_path_cross_block(&root_tree, &block_contents, &uri("inner-1"))
            .expect("should find inner-1 across block boundary");

        let input = WidgetInput::Navigate {
            direction: NavDirection::Down,
            hint: CursorHint {
                column: 0,
                boundary: Boundary::Bottom,
            },
        };

        match fp.bubble_input(&uri("inner-1"), &input) {
            Some(InputAction::Focus { block_id, .. }) => {
                assert_eq!(block_id, uri("inner-2"));
            }
            other => panic!("expected Focus to inner-2, got {other:?}"),
        }
    }

    #[test]
    fn collect_all_entity_ids_traverses_tree() {
        let tree = column(vec![make_row("a"), row(vec![make_row("b"), make_row("c")])]);
        let ids = collect_all_entity_ids(&tree);
        assert!(ids.contains(&uri("a")));
        assert!(ids.contains(&uri("b")));
        assert!(ids.contains(&uri("c")));
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
        match router.bubble_input(&uri("a"), &input) {
            Some(InputAction::Focus { block_id, .. }) => assert_eq!(block_id, uri("b")),
            other => panic!("expected Focus to b, got {other:?}"),
        }

        // After navigation resolved to "b", calling again with "b" should
        // reuse the prefix and navigate to "c"
        match router.bubble_input(&uri("b"), &input) {
            Some(InputAction::Focus { block_id, .. }) => assert_eq!(block_id, uri("c")),
            other => panic!("expected Focus to c, got {other:?}"),
        }

        // Last element: None
        assert!(router.bubble_input(&uri("c"), &input).is_none());
    }

    #[test]
    fn find_click_intent_oneshot_returns_bound_action() {
        use holon_api::EntityName;
        use holon_api::render_types::OperationDescriptor;
        use holon_api::render_types::OperationWiring;
        use holon_api::render_types::Trigger;

        // Sidebar-shaped tree: list → selectable(row(text(...))) per item.
        // Each selectable carries a click-bound `navigation.focus` op.
        let mut sidebar_item = make_row("block:page-foo");
        sidebar_item.operations.push(OperationWiring {
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: EntityName::new("navigation"),
                name: "focus".into(),
                trigger: Some(Trigger::Click {
                    modifiers: holon_api::ClickModifiers::none(),
                }),
                bound_params: StdHashMap::from([
                    ("region".into(), Value::String("main".into())),
                    ("block_id".into(), Value::String("block:page-foo".into())),
                ]),
                entity_short_name: String::new(),
                id_column: "id".to_string(),
                display_name: String::new(),
                description: String::new(),
                required_params: vec![],
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::Test,
                },
                marking_delta: holon_api::marking::MarkingDelta::Undeclared,
                guard: holon_api::pattern::OpGuard::None,
                arcs: holon_api::arcs::TransitionArcs::Undeclared,
            },
        });

        let tree = list(vec![sidebar_item, make_row("block:page-bar")]);

        // Click on the item with a bound action → returns the navigation.focus intent.
        let intent = find_click_intent_oneshot(&tree, &uri("block:page-foo"))
            .expect("block:page-foo should yield a click intent");
        assert_eq!(intent.entity_name.as_str(), "navigation");
        assert_eq!(intent.op_name, "focus");
        assert_eq!(
            intent.params.get("block_id").and_then(|v| v.as_string()),
            Some("block:page-foo")
        );

        // Click on the item without a bound action → None (driver falls back).
        assert!(find_click_intent_oneshot(&tree, &uri("block:page-bar")).is_none());

        // Click on a non-existent entity → None.
        assert!(find_click_intent_oneshot(&tree, &uri("block:page-nope")).is_none());
    }

    #[test]
    fn find_click_intent_in_region_scopes_to_panel() {
        // Same entity_id ("block:foo") appears in BOTH panels. Only the
        // LeftSidebar's wrapper carries a click-bound nav.focus action;
        // the Main panel wrapper has no operations. The unscoped walker
        // returns the LeftSidebar action regardless of clicked region —
        // we want the scoped walker to honor the region.
        use holon_api::EntityName;
        use holon_api::render_types::OperationDescriptor;
        use holon_api::render_types::OperationWiring;
        use holon_api::render_types::Trigger;
        use holon_api::widget_spec::DataRow;

        use crate::view_model::ViewModel;

        fn entity_row(id: &str) -> Arc<DataRow> {
            Arc::new(StdHashMap::from([("id".into(), Value::String(id.into()))]))
        }

        let mut sidebar_item = ViewModel::element("table_row", entity_row("block:foo"), vec![]);
        sidebar_item.operations.push(OperationWiring {
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: EntityName::new("navigation"),
                name: "focus".into(),
                trigger: Some(Trigger::Click {
                    modifiers: holon_api::ClickModifiers::none(),
                }),
                bound_params: StdHashMap::from([
                    ("region".into(), Value::String("main".into())),
                    ("block_id".into(), Value::String("block:foo".into())),
                ]),
                entity_short_name: String::new(),
                id_column: "id".to_string(),
                display_name: String::new(),
                description: String::new(),
                required_params: vec![],
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::Test,
                },
                marking_delta: holon_api::marking::MarkingDelta::Undeclared,
                guard: holon_api::pattern::OpGuard::None,
                arcs: holon_api::arcs::TransitionArcs::Undeclared,
            },
        });

        let main_item = ViewModel::element("table_row", entity_row("block:foo"), vec![]);

        // Build the layout: columns(panel(left_sidebar), panel(main_panel)).
        // The panel wrappers are `live_block` nodes whose entity_id matches
        // the panel's well-known id.
        let left_panel = ViewModel::live_block(
            "block:default-left-sidebar",
            ViewModel::collection("list", vec![sidebar_item]),
        );
        let main_panel = ViewModel::live_block(
            "block:default-main-panel",
            // `collection` has no `column` kind; this resolved to `list` via the
            // old silent default, so spell it `list` now that parsing is strict.
            ViewModel::collection("list", vec![main_item]),
        );
        let root = ViewModel::layout("columns", vec![left_panel, main_panel]);

        // LeftSidebar click on block:foo → fires the bound nav.focus.
        let left_intent = find_click_intent_in_region(
            &root,
            &uri("block:foo"),
            "left_sidebar",
            holon_api::ClickModifiers::none(),
        )
        .expect("left_sidebar click on block:foo should yield an intent");
        assert_eq!(left_intent.entity_name.as_str(), "navigation");
        assert_eq!(left_intent.op_name, "focus");

        // Main click on the same block:foo → no bound action in the Main
        // panel's subtree. Returns None; production would fall through to
        // editor_focus.
        assert!(
            find_click_intent_in_region(
                &root,
                &uri("block:foo"),
                "main",
                holon_api::ClickModifiers::none()
            )
            .is_none(),
            "Main click on block:foo must NOT pick up the LeftSidebar's bound action"
        );

        // Unknown region → None (defensive).
        assert!(
            find_click_intent_in_region(
                &root,
                &uri("block:foo"),
                "bogus_region",
                holon_api::ClickModifiers::none()
            )
            .is_none()
        );

        // Entity not in any panel's subtree → None.
        assert!(
            find_click_intent_in_region(
                &root,
                &uri("block:never"),
                "left_sidebar",
                holon_api::ClickModifiers::none()
            )
            .is_none()
        );
    }
}
