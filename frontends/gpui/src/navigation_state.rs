use std::sync::{Arc, RwLock};

use holon_frontend::input::{InputAction, WidgetInput};
use holon_frontend::shadow_index::IncrementalShadowIndex;
use holon_mcp::server::NavigationDebugState;
use std::sync::RwLock as StdRwLock;

use crate::entity_view_registry::FocusRegistry;

/// Navigation state: shadow index + debug state for MCP inspection.
#[derive(Clone)]
pub struct NavigationState {
    shadow_index: Arc<RwLock<Option<IncrementalShadowIndex>>>,
    navigation_debug: Arc<StdRwLock<NavigationDebugState>>,
}

impl NavigationState {
    pub fn new() -> Self {
        Self::with_shadow_index(Arc::new(RwLock::new(None)))
    }

    pub fn with_shadow_index(shadow_index: Arc<RwLock<Option<IncrementalShadowIndex>>>) -> Self {
        Self {
            shadow_index,
            navigation_debug: Arc::new(StdRwLock::new(NavigationDebugState::default())),
        }
    }

    pub fn set_navigation_debug(&mut self, state: Arc<StdRwLock<NavigationDebugState>>) {
        self.navigation_debug = state;
    }

    #[tracing::instrument(level = "debug", skip_all)]
    pub fn set_shadow_index(&self, index: IncrementalShadowIndex, focus: &FocusRegistry) {
        let desc = index.describe();
        let editor_ids = focus.editor_inputs.keys();
        let entity_desc = focus.describe_editor_inputs();
        if let Ok(mut state) = self.navigation_debug.write() {
            state.shadow_index_description = desc;
            state.editor_input_ids = editor_ids;
            state.entity_registry_description = entity_desc;
        }
        *self.shadow_index.write().unwrap() = Some(index);
    }

    #[tracing::instrument(level = "debug", skip_all, fields(block_id))]
    pub fn patch_shadow_block(
        &self,
        block_id: &str,
        new_content: &holon_frontend::reactive_view_model::ReactiveViewModel,
        focus: &FocusRegistry,
    ) {
        if let Some(ref mut index) = *self.shadow_index.write().unwrap() {
            index.patch_block(block_id, new_content);
            let desc = index.describe();
            let editor_ids = focus.editor_inputs.keys();
            let entity_desc = focus.describe_editor_inputs();
            if let Ok(mut state) = self.navigation_debug.write() {
                state.shadow_index_description = desc;
                state.editor_input_ids = editor_ids;
                state.entity_registry_description = entity_desc;
            }
        }
    }

    #[tracing::instrument(level = "debug", skip_all, fields(entity_id))]
    pub fn bubble_input(&self, entity_id: &str, input: &WidgetInput) -> Option<InputAction> {
        self.shadow_index
            .read()
            .unwrap()
            .as_ref()?
            .bubble_input(entity_id, input)
    }

    pub fn shadow_entity_count(&self) -> usize {
        self.shadow_index
            .read()
            .unwrap()
            .as_ref()
            .map(|i| i.entity_ids().len())
            .unwrap_or(0)
    }

    pub fn has_shadow_index(&self) -> bool {
        self.shadow_index.read().unwrap().is_some()
    }

    pub fn describe_shadow_index(&self) -> String {
        match self.shadow_index.read().unwrap().as_ref() {
            Some(index) => index.describe(),
            None => "Shadow index: None".to_string(),
        }
    }
}
