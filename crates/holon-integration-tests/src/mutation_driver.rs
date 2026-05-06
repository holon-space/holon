//! `DirectUserDriver` — legacy PBT driver that bypasses FrontendSession and
//! calls `BackendEngine::execute_operation` directly. Used by backend PBTs
//! that don't need the reactive/UI pipeline.
//!
//! The `UserDriver` trait and `ReactiveEngineDriver` now live in
//! `holon_frontend::user_driver` so they can be shared across all
//! frontends (including MCP's channel-based `GpuiUserDriver`). This module
//! re-exports them for backcompat with existing test code.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon_api::{EntityName, KeyChord, Value};
use holon_frontend::ReactiveViewModel;
use holon_frontend::operations::OperationIntent;

pub use holon_frontend::user_driver::{ReactiveEngineDriver, UserDriver};

/// Dispatches mutations directly via `BackendEngine::execute_operation`.
/// Legacy driver — bypasses FrontendSession and ReactiveEngine.
pub struct DirectUserDriver {
    engine: Arc<BackendEngine>,
}

impl DirectUserDriver {
    pub fn new(engine: Arc<BackendEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait::async_trait]
impl UserDriver for DirectUserDriver {
    async fn synthetic_dispatch(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()> {
        self.engine
            .execute_operation(&EntityName::new(entity), op, params)
            .await
            .map(|_| ())
            .context(format!("execute_operation({entity}, {op}) failed"))
    }

    /// Drag&drop has no faithful direct-engine equivalent — `DirectUserDriver`
    /// bypasses the reactive layer where draggable / drop_zone widgets live.
    /// Tests that need drag&drop must install a driver with widget-tree
    /// access (e.g. `ReactiveEngineDriver` or `GpuiUserDriver`).
    async fn drop_entity(&self, _: &str, _: &str, _: &str) -> Result<bool> {
        anyhow::bail!(
            "DirectUserDriver does not implement drop_entity — install \
             ReactiveEngineDriver or a native frontend driver to exercise \
             drag&drop transitions"
        )
    }

    // ── Action verbs ────────────────────────────────────────────────────
    //
    // Backend-direct equivalents of the user actions. These keep the
    // historical "fast PBT" behavior for tests that don't need the
    // reactive pipeline — they're the same bodies the trait defaults used
    // to provide, just made explicit so screen drivers can't accidentally
    // inherit them.

    async fn send_key_chord(
        &self,
        _: &str,
        root_tree: &ReactiveViewModel,
        entity_id: &str,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        use holon_frontend::input::{InputAction, WidgetInput};
        let input = WidgetInput::KeyChord {
            keys: chord.0.clone(),
        };
        let action = holon_frontend::focus_path::bubble_input_oneshot(root_tree, entity_id, &input);
        match action {
            Some(InputAction::ExecuteOperation {
                entity_name,
                operation,
                entity_id,
            }) => {
                let mut params = HashMap::new();
                params.insert("id".into(), Value::String(entity_id));
                params.extend(extra_params);
                self.synthetic_dispatch(entity_name.as_str(), &operation.name, params)
                    .await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn click_entity(&self, entity_id: &str, region: &str) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("region".into(), Value::String(region.to_string()));
        params.insert("block_id".into(), Value::String(entity_id.to_string()));
        params.insert("cursor_offset".into(), Value::Integer(0));
        self.synthetic_dispatch("navigation", "editor_focus", params)
            .await
    }

    async fn click_entity_with_tree(
        &self,
        _: &str,
        root_tree: &ReactiveViewModel,
        entity_id: &str,
        region: &str,
    ) -> Result<bool> {
        if let Some(intent) =
            holon_frontend::focus_path::find_click_intent_oneshot(root_tree, entity_id)
        {
            self.apply_intent(intent).await?;
            return Ok(true);
        }
        self.click_entity(entity_id, region).await?;
        Ok(false)
    }

    async fn type_text(&self, entity_id: &str, text: &str) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("id".into(), Value::String(entity_id.to_string()));
        params.insert("content".into(), Value::String(text.to_string()));
        self.synthetic_dispatch("block", "update", params).await
    }

    // ── Observation verbs ───────────────────────────────────────────────
    //
    // DirectUserDriver has no reactive state, so it can't faithfully
    // answer "what's visible" or "what's clickable". Bail loudly. Tests
    // that need observation must install ReactiveEngineDriver.

    fn is_widget_visible(&self, _: &str) -> bool {
        false
    }

    fn is_in_region(&self, _: &str, _: holon_api::Region) -> bool {
        false
    }

    fn entities_in_region(&self, _: holon_api::Region) -> Vec<holon_api::EntityUri> {
        Vec::new()
    }

    fn reachable_entities_in_region(&self, _: holon_api::Region) -> Vec<holon_api::EntityUri> {
        Vec::new()
    }

    async fn scroll_to_entity(&self, _: &str) -> Result<()> {
        Ok(())
    }

    fn click_intent_of(&self, _: &str) -> Option<OperationIntent> {
        None
    }

    fn displayed_text(&self, _: &str) -> Option<String> {
        None
    }
}
