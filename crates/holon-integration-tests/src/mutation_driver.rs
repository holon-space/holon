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
use holon_api::{EntityName, Value};

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
    async fn drop_entity(
        &self,
        _root_block_id: &str,
        _source_id: &str,
        _target_id: &str,
    ) -> Result<bool> {
        anyhow::bail!(
            "DirectUserDriver does not implement drop_entity — install \
             ReactiveEngineDriver or a native frontend driver to exercise \
             drag&drop transitions"
        )
    }
}
