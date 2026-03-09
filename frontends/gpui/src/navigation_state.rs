use std::sync::Arc;

use holon_frontend::focus_path::InputRouter;
use holon_frontend::input::{InputAction, WidgetInput};
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_mcp::server::NavigationDebugState;
use std::sync::RwLock as StdRwLock;

use crate::entity_view_registry::FocusRegistry;

/// Navigation state: input router + debug state for MCP inspection.
#[derive(Clone)]
pub struct NavigationState {
    input_router: Arc<InputRouter>,
    navigation_debug: Arc<StdRwLock<NavigationDebugState>>,
}

impl NavigationState {
    pub fn new() -> Self {
        Self {
            input_router: Arc::new(InputRouter::new()),
            navigation_debug: Arc::new(StdRwLock::new(NavigationDebugState::default())),
        }
    }

    /// Create with a shared InputRouter (used to share with MCP DebugServices).
    pub fn with_input_router(input_router: Arc<InputRouter>) -> Self {
        Self {
            input_router,
            navigation_debug: Arc::new(StdRwLock::new(NavigationDebugState::default())),
        }
    }

    pub fn set_navigation_debug(&mut self, state: Arc<StdRwLock<NavigationDebugState>>) {
        self.navigation_debug = state;
    }

    #[tracing::instrument(level = "debug", skip_all)]
    pub fn set_root(&self, root_tree: Arc<ReactiveViewModel>, focus: &FocusRegistry) {
        self.input_router.set_root(root_tree);
        let desc = self.input_router.describe();
        let editor_ids = focus.editor_inputs.keys();
        let entity_desc = focus.describe_editor_inputs();
        if let Ok(mut state) = self.navigation_debug.write() {
            state.tree_description = desc;
            state.editor_input_ids = editor_ids;
            state.entity_registry_description = entity_desc;
        }
    }

    /// Install a `LiveBlockResolver` on the input router. Without this,
    /// chord ops on widgets inside any nested `live_block` (every Main-panel
    /// block, drawer content, etc.) silently no-op because `nav.set_root`'s
    /// tree carries empty live_block slots — the resolver bridges the
    /// router into per-block reactive trees.
    pub fn set_block_resolver(&self, resolver: holon_frontend::focus_path::LiveBlockResolver) {
        self.input_router.set_block_resolver(resolver);
    }

    #[tracing::instrument(level = "debug", skip_all, fields(entity_id))]
    pub fn bubble_input(&self, entity_id: &str, input: &WidgetInput) -> Option<InputAction> {
        self.input_router.bubble_input(entity_id, input)
    }

    pub fn has_root(&self) -> bool {
        self.input_router.has_root()
    }

    pub fn describe(&self) -> String {
        self.input_router.describe()
    }
}
