use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use gpui::{AnyEntity, App, Entity, Focusable as _, Window};
use gpui_component::input::InputState;

use crate::views::{LiveQueryView, RenderEntityView};

// ── Builder entity types ────────────────────────────────────────────────

/// Self-rendering collapsible disclosure widget. `impl Render` is in `builders/collapsible.rs`.
pub struct CollapsibleView {
    pub collapsed: bool,
    pub header_text: String,
    pub icon_text: String,
    pub detail_text: String,
}

/// Simple boolean toggle state shared by tree items and pie menus.
pub struct ToggleState {
    pub active: bool,
}

// ── ViewRegistry<T> ─────────────────────────────────────────────────────

/// Generic, cloneable registry for GPUI Entity handles keyed by String.
pub struct ViewRegistry<T: 'static>(Arc<RwLock<HashMap<String, Entity<T>>>>);

impl<T: 'static> Clone for ViewRegistry<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: 'static> ViewRegistry<T> {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }

    pub fn get(&self, key: &str) -> Option<Entity<T>> {
        self.0.read().unwrap().get(key).cloned()
    }

    pub fn register(&self, key: String, entity: Entity<T>) {
        self.0.write().unwrap().insert(key, entity);
    }

    pub fn unregister(&self, key: &str) {
        self.0.write().unwrap().remove(key);
    }

    pub fn keys(&self) -> Vec<String> {
        self.0.read().unwrap().keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.0.read().unwrap().len()
    }

    pub fn iter_entries(&self) -> Vec<(String, Entity<T>)> {
        self.0
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

// ── FocusRegistry ───────────────────────────────────────────────────────

/// Global registry for editor entities — survives structural rebuilds.
/// Required because cross-block navigation must scan ALL editors to find focus,
/// and editor entities must persist even when the widget tree is rebuilt.
#[derive(Clone)]
pub struct FocusRegistry {
    pub editor_inputs: ViewRegistry<InputState>,
    pub editor_views: ViewRegistry<crate::views::EditorView>,
}

impl FocusRegistry {
    pub fn new() -> Self {
        Self {
            editor_inputs: ViewRegistry::new(),
            editor_views: ViewRegistry::new(),
        }
    }

    /// Find which editor's InputState is currently focused, returning its row_id.
    #[tracing::instrument(level = "debug", skip_all)]
    pub fn focused_editor_row_id(&self, window: &Window, cx: &App) -> Option<String> {
        for (row_id, input) in self.editor_inputs.iter_entries() {
            if input.read(cx).focus_handle(cx).is_focused(window) {
                return Some(row_id);
            }
        }
        None
    }

    /// Get the cursor column of the focused editor (for building CursorHint).
    pub fn focused_cursor_column(&self, row_id: &str, cx: &App) -> usize {
        if let Some(input) = self.editor_inputs.get(row_id) {
            input.read(cx).cursor_position().character as usize
        } else {
            0
        }
    }

    /// Get the byte cursor offset of the given editor.
    pub fn focused_cursor_byte(&self, row_id: &str, cx: &App) -> usize {
        self.editor_inputs
            .get(row_id)
            .map(|input| input.read(cx).cursor())
            .unwrap_or(0)
    }

    /// List all editor_input row_ids for debugging.
    pub fn describe_editor_inputs(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let keys = self.editor_inputs.keys();
        writeln!(out, "Editor inputs: {} entries", keys.len()).ok();
        for row_id in keys {
            writeln!(out, "  {row_id}").ok();
        }
        out
    }
}

// ── LocalEntityScope ────────────────────────────────────────────────────

/// Entity cache for builder-created widgets (toggles, collapsibles).
/// Arc-owned by the parent view, persists across re-renders.
pub type EntityCache = Arc<RwLock<HashMap<String, AnyEntity>>>;

/// Parent-scoped entity context, built fresh each render pass.
///
/// The 5 entity HashMaps are read-only snapshots from the parent's local state.
/// The `entity_cache` persists builder-created state (toggles, collapsibles)
/// across re-renders via the parent-owned Arc.
pub struct LocalEntityScope {
    pub render_entitys: HashMap<String, Entity<RenderEntityView>>,
    pub live_queries: HashMap<String, Entity<LiveQueryView>>,
    pub(crate) entity_cache: EntityCache,
}

impl LocalEntityScope {
    pub fn new() -> Self {
        Self {
            render_entitys: HashMap::new(),
            live_queries: HashMap::new(),
            entity_cache: Default::default(),
        }
    }

    pub fn with_cache(mut self, cache: EntityCache) -> Self {
        self.entity_cache = cache;
        self
    }

    /// Get or create a cached entity by stable key.
    /// Persists across re-renders because the parent view owns the EntityCache Arc.
    pub fn get_or_create(&self, key: &str, create: impl FnOnce() -> AnyEntity) -> AnyEntity {
        let mut cache = self.entity_cache.write().unwrap();
        cache.entry(key.to_string()).or_insert_with(create).clone()
    }
}
