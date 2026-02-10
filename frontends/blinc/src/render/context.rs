use std::sync::Arc;

use blinc_core::State;
use holon_api::render_types::OperationWiring;
use holon_api::widget_spec::DataRow;
use holon_frontend::{FrontendSession, RenderPipeline};

#[derive(Clone)]
pub struct BlincExt {
    pub sidebar_open: Option<State<bool>>,
    pub right_sidebar_open: Option<State<bool>>,
    pub focused_block_id: Option<State<Option<String>>>,
    /// Block ID of the left sidebar region (set during screen layout render).
    /// Used by the title bar toggle to persist open/closed state.
    pub left_sidebar_block_id: Option<State<Option<String>>>,
}

impl Default for BlincExt {
    fn default() -> Self {
        Self {
            sidebar_open: None,
            right_sidebar_open: None,
            focused_block_id: None,
            left_sidebar_block_id: None,
        }
    }
}

/// Blinc-specific render context wrapping the shared RenderContext with Blinc extensions.
#[derive(Clone)]
pub struct RenderContext {
    pub ctx: holon_frontend::RenderContext,
    pub ext: BlincExt,
}

impl std::ops::Deref for RenderContext {
    type Target = holon_frontend::RenderContext;
    fn deref(&self) -> &Self::Target {
        &self.ctx
    }
}

impl RenderContext {
    /// Delegate with_operations to inner, preserving ext.
    pub fn with_operations(&self, operations: Vec<OperationWiring>) -> Self {
        Self { ctx: self.ctx.with_operations(operations), ext: self.ext.clone() }
    }

    /// Delegate with_row to inner, preserving ext.
    pub fn with_row(&self, row: DataRow) -> Self {
        Self { ctx: self.ctx.with_row(row), ext: self.ext.clone() }
    }

    /// Delegate with_data_rows to inner, preserving ext.
    pub fn with_data_rows(&self, data_rows: Vec<DataRow>) -> Self {
        Self { ctx: self.ctx.with_data_rows(data_rows), ext: self.ext.clone() }
    }

    /// Delegate deeper_query to inner, preserving ext.
    pub fn deeper_query(&self) -> Self {
        Self { ctx: self.ctx.deeper_query(), ext: self.ext.clone() }
    }

    /// Delegate indented to inner, preserving ext.
    pub fn indented(&self) -> Self {
        Self { ctx: self.ctx.indented(), ext: self.ext.clone() }
    }
}

pub fn new_render_context(
    session: Arc<FrontendSession>,
    runtime_handle: tokio::runtime::Handle,
    block_watch: holon_frontend::BlockWatchRegistry,
) -> RenderContext {
    let widget_states = Arc::new(session.ui_settings().widgets);
    let pipeline = Arc::new(RenderPipeline {
        session,
        runtime_handle,
        block_watch,
        widget_states,
    });
    RenderContext {
        ctx: holon_frontend::RenderContext::from_pipeline(pipeline),
        ext: BlincExt::default(),
    }
}
