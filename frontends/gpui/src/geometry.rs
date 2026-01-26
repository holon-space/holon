//! GPUI GeometryProvider — reads from a shared BoundsRegistry populated during render.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use gpui::{Bounds, Entity, Pixels};
use holon_frontend::geometry::{ElementRect, GeometryProvider};

use crate::views::{BlockRefView, EditorView, LiveQueryView};

/// Shared registry of element bounds, populated during GPUI render passes.
///
/// Also caches a snapshot of gpui-component's `ThemeColor` so builders can
/// read theme tokens via `tc()` without needing `&App` access.
#[derive(Clone)]
pub struct BoundsRegistry {
    inner: Arc<RwLock<HashMap<String, Bounds<Pixels>>>>,
    theme: Arc<RwLock<gpui_component::theme::ThemeColor>>,
    /// Set of currently open pie menu element IDs.
    open_pie_menus: Arc<RwLock<std::collections::HashSet<String>>>,
    /// Set of collapsed tree item IDs. Items not in this set are expanded (default).
    collapsed_tree_items: Arc<RwLock<std::collections::HashSet<String>>>,
    /// Pre-created BlockRefView entities, keyed by block_id.
    block_views: Arc<RwLock<HashMap<String, Entity<BlockRefView>>>>,
    /// Pre-created EditorView entities, keyed by element ID.
    editor_views: Arc<RwLock<HashMap<String, Entity<EditorView>>>>,
    /// Pre-created LiveQueryView entities, keyed by query hash.
    live_query_views: Arc<RwLock<HashMap<String, Entity<LiveQueryView>>>>,
}

impl BoundsRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            theme: Arc::new(RwLock::new(gpui_component::theme::ThemeColor::default())),
            open_pie_menus: Arc::new(RwLock::new(std::collections::HashSet::new())),
            collapsed_tree_items: Arc::new(RwLock::new(std::collections::HashSet::new())),
            block_views: Arc::new(RwLock::new(HashMap::new())),
            editor_views: Arc::new(RwLock::new(HashMap::new())),
            live_query_views: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Snapshot the current gpui-component Theme global into this registry.
    /// Call at the start of each render pass.
    pub fn sync_theme(&self, cx: &gpui::App) {
        use gpui_component::theme::ActiveTheme;
        let colors = cx.theme().colors;
        *self.theme.write().unwrap() = colors;
    }

    pub fn theme(&self) -> gpui_component::theme::ThemeColor {
        *self.theme.read().unwrap()
    }

    /// Whether icons should render in greyscale (dimmed) mode.
    /// Returns false (full color) by default; will be driven by focus/mode state later.
    pub fn icon_greyscale(&self) -> bool {
        false
    }

    /// Record the bounds of an element after layout.
    pub fn record(&self, id: String, bounds: Bounds<Pixels>) {
        self.inner.write().unwrap().insert(id, bounds);
    }

    /// Clear all recorded bounds (call at the start of each render pass).
    pub fn clear(&self) {
        self.inner.write().unwrap().clear();
    }

    pub fn is_pie_menu_open(&self, id: &str) -> bool {
        self.open_pie_menus.read().unwrap().contains(id)
    }

    pub fn toggle_pie_menu(&self, id: String) {
        let mut set = self.open_pie_menus.write().unwrap();
        if !set.remove(&id) {
            set.clear(); // close any other open pie menu
            set.insert(id);
        }
    }

    pub fn close_pie_menu(&self, id: &str) {
        self.open_pie_menus.write().unwrap().remove(id);
    }

    pub fn close_all_pie_menus(&self) {
        self.open_pie_menus.write().unwrap().clear();
    }

    pub fn is_tree_item_collapsed(&self, id: &str) -> bool {
        self.collapsed_tree_items.read().unwrap().contains(id)
    }

    pub fn toggle_tree_item_collapsed(&self, id: String) {
        let mut set = self.collapsed_tree_items.write().unwrap();
        if !set.remove(&id) {
            set.insert(id);
        }
    }

    /// Get a BlockRefView entity by block_id.
    pub fn get_block_view(&self, block_id: &str) -> Option<Entity<BlockRefView>> {
        self.block_views.read().unwrap().get(block_id).cloned()
    }

    /// Replace the entire block_views map (called after reconciliation).
    pub fn set_block_views(&self, views: HashMap<String, Entity<BlockRefView>>) {
        *self.block_views.write().unwrap() = views;
    }

    /// Get an EditorView entity by element ID.
    pub fn get_editor_view(&self, el_id: &str) -> Option<Entity<EditorView>> {
        self.editor_views.read().unwrap().get(el_id).cloned()
    }

    /// Replace the entire editor_views map (called after reconciliation).
    pub fn set_editor_views(&self, views: HashMap<String, Entity<EditorView>>) {
        *self.editor_views.write().unwrap() = views;
    }

    /// Get a LiveQueryView entity by query key.
    pub fn get_live_query_view(&self, key: &str) -> Option<Entity<LiveQueryView>> {
        self.live_query_views.read().unwrap().get(key).cloned()
    }

    /// Replace the entire live_query_views map (called after reconciliation).
    pub fn set_live_query_views(&self, views: HashMap<String, Entity<LiveQueryView>>) {
        *self.live_query_views.write().unwrap() = views;
    }
}

impl GeometryProvider for BoundsRegistry {
    fn element_bounds(&self, id: &str) -> Option<ElementRect> {
        let map = self.inner.read().unwrap();
        map.get(id).map(|b| ElementRect {
            x: f32::from(b.origin.x),
            y: f32::from(b.origin.y),
            width: f32::from(b.size.width),
            height: f32::from(b.size.height),
        })
    }

    fn all_element_ids(&self) -> Vec<String> {
        self.inner.read().unwrap().keys().cloned().collect()
    }
}
