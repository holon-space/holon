//! Render-harness component for the PBT SUT.
//!
//! Owns the rendering-side state of `E2ESut`:
//! - the SUT's own headless `ReactiveEngine` (`reactive_engine`,
//!   `reactive_root_id`) plus the `vm_emissions` stream collected from it;
//! - the externally-injected GPUI frontend surfaces (`frontend_engine`,
//!   `frontend_geometry`, `frontend_visual_state`) that the phased/GPUI
//!   harness installs after StartApp;
//! - the headless `live_tree` mirror.
//!
//! The genuinely pure helpers that touch only this state live here. The
//! ctx-coupled helpers (`ensure_reactive_engine`, the `wait_for_*` family,
//! the chord senders) stay on `E2ESut` and read `self.render.*` — they also
//! need the engine, driver, and DI reactive engine. Fields are `pub(super)`
//! so those `E2ESut` methods and the invariant capability impls can reach
//! them directly; `E2ESut` keeps same-signature forwarding methods for the
//! pure helpers so their call sites are unchanged.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use holon_api::EntityUri;

/// Rendering-side state of the SUT. See module docs.
pub(super) struct RenderSut {
    /// Reactive engine for root layout — the SUT's own instance, created
    /// lazily by `E2ESut::ensure_reactive_engine`. `RefCell` because
    /// `check_invariants` receives `&self`.
    pub(super) reactive_engine: RefCell<Option<Arc<holon_frontend::reactive::ReactiveEngine>>>,
    /// Root layout block ID used by the ReactiveEngine — set during StartApp,
    /// used by `current_reactive_tree()` and the headless `wait_for_entity_*`
    /// helpers.
    pub(super) reactive_root_id: RefCell<Option<EntityUri>>,
    /// Every ViewModel emission from the reactive stream, collected by a
    /// background task. check_invariants drains this and checks each
    /// intermediate ViewModel — catches transient CDC bugs masked by
    /// structural re-renders.
    pub(super) vm_emissions: Arc<Mutex<Vec<holon_frontend::ViewModel>>>,
    /// Optional external frontend engine (e.g. GPUI's ReactiveEngine).
    /// When set, inv-frontend-engine checks the frontend's own ViewModel for
    /// errors.
    pub(super) frontend_engine: Option<Arc<holon_frontend::reactive::ReactiveEngine>>,
    /// When set, inv-frontend-engine also checks that GPUI actually laid out
    /// the expected elements.
    pub(super) frontend_geometry: Option<Box<dyn holon_frontend::geometry::GeometryProvider>>,
    /// Shared screenshot analysis — the GeometryDriver updates this after each
    /// screenshot, and inv-frontend-engine reads it to assert the UI isn't
    /// visually empty.
    pub(super) frontend_visual_state: Option<crate::ui_driver::VisualState>,
    /// Headless live tree — persistent collection backed by the engine's live
    /// CDC data. Mirrors what the GPUI frontend sees: the collection driver
    /// calls `set_data` on existing items when data changes. Compared against
    /// the fresh tree in check_invariants to catch set_data propagation bugs.
    pub(super) live_tree: RefCell<Option<holon_layout_testing::live_tree::HeadlessLiveTree>>,
}

impl RenderSut {
    pub(super) fn new() -> Self {
        Self {
            reactive_engine: RefCell::new(None),
            reactive_root_id: RefCell::new(None),
            vm_emissions: Arc::new(Mutex::new(Vec::new())),
            frontend_engine: None,
            frontend_geometry: None,
            frontend_visual_state: None,
            live_tree: RefCell::new(None),
        }
    }

    /// Install the externally-built GPUI frontend surfaces after StartApp.
    /// Called by the phased harness once the `on_ready`/callback hook has
    /// produced them (or `None` for headless runs).
    pub(super) fn install_frontend(
        &mut self,
        engine: Option<Arc<holon_frontend::reactive::ReactiveEngine>>,
        geometry: Option<Box<dyn holon_frontend::geometry::GeometryProvider>>,
        visual_state: Option<crate::ui_driver::VisualState>,
    ) {
        self.frontend_engine = engine;
        self.frontend_geometry = geometry;
        self.frontend_visual_state = visual_state;
    }

    /// Snapshot the current root layout as a `ReactiveViewModel` — the input
    /// the trait-level `send_key_chord` / `resolve_key_chord` needs.
    pub(super) fn current_reactive_tree(
        &self,
    ) -> Option<(holon_api::EntityUri, holon_frontend::ReactiveViewModel)> {
        let engine = self.reactive_engine.borrow();
        let engine = engine.as_ref()?;
        let root_id = self
            .reactive_root_id
            .borrow()
            .clone()
            .unwrap_or_else(holon_api::root_layout_block_uri);
        Some((root_id.clone(), engine.snapshot_reactive(&root_id)))
    }

    /// Walk the live reactive tree from `root` and flip the
    /// `expanded` Mutable of the `expand_toggle` whose `target_id`
    /// prop matches `block_id`. Used by `apply_expand_toggle` /
    /// `apply_collapse_toggle`.
    ///
    /// Note on headless persistence: `ReactiveEngine::snapshot_reactive`
    /// runs `interpret_fn` and returns a freshly-built tree on every
    /// call, so the `Mutable<bool>` we flip here is reborn on the next
    /// snapshot unless the GPUI-side `with_update` / `push_down_*`
    /// machinery is holding a persistent root. This SUT helper is
    /// correct in either case: the reference-model side already tracks
    /// the toggle in `state.expanded_toggles`, and the assertion below
    /// fails loud if the corpus grows an `expand_toggle` render but the
    /// engine never produces a matching node (the most likely
    /// regression).
    pub(super) async fn set_expand_toggle_gate(
        &self,
        block_id: &holon_api::EntityUri,
        value: bool,
    ) {
        use holon_frontend::reactive_view_model::ReactiveViewModel;

        let engine = self
            .reactive_engine
            .borrow()
            .clone()
            .expect("reactive engine not installed — was start_app called?");
        let root_id = self
            .reactive_root_id
            .borrow()
            .clone()
            .unwrap_or_else(holon_api::root_layout_block_uri);
        let root = engine.snapshot_reactive(&root_id);

        fn find_and_flip(node: &ReactiveViewModel, target_id: &str, value: bool) -> bool {
            let is_toggle = matches!(node.widget_name().as_deref(), Some("expand_toggle"));
            if is_toggle {
                let props = node.props.lock_ref();
                let matches = props
                    .get("target_id")
                    .and_then(|v| v.as_string())
                    .map(|s| s == target_id)
                    .unwrap_or(false);
                drop(props);
                if matches && let Some(gate) = node.expanded.as_ref() {
                    gate.set(value);
                    return true;
                }
            }
            for child in &node.children {
                if find_and_flip(child, target_id, value) {
                    return true;
                }
            }
            if let Some(slot) = node.slot.as_ref() {
                let content = slot.content.lock_ref();
                if find_and_flip(&content, target_id, value) {
                    return true;
                }
            }
            if let Some(lazy) = node.lazy_slot.as_ref()
                && let Some(materialised) = lazy.cache.get_cloned()
                && find_and_flip(&materialised, target_id, value)
            {
                return true;
            }
            false
        }

        let block_uri = block_id.to_string();
        let target_id = block_uri.strip_prefix("block:").unwrap_or(&block_uri);
        assert!(
            find_and_flip(&root, target_id, value),
            "set_expand_toggle_gate: no expand_toggle node with \
             target_id={target_id} in reactive tree under {root_id}. \
             The fixture grew an expand_toggle render but the engine \
             didn't produce a matching node — likely a shadow_builder \
             or interpret regression. See \
             devlog/2026-05-15-lazy-expand-toggle-plan.md."
        );
    }

    /// Look up the keybinding for an operation name from the reactive engine's registry.
    pub(super) fn find_keybinding_for_op(&self, op_name: &str) -> Option<holon_api::KeyChord> {
        let engine = self.reactive_engine.borrow();
        let engine = engine.as_ref()?;
        engine.key_bindings().lock_ref().get(op_name).cloned()
    }

    /// Drain all pending ViewModel emissions since the last drain.
    pub(super) fn vm_emissions_drain(&self) -> Vec<holon_frontend::ViewModel> {
        std::mem::take(&mut *self.vm_emissions.lock().unwrap())
    }
}
