use std::sync::{Arc, Mutex};

use holon_api::widget_spec::DataRow;
use holon_frontend::{FrontendSession, RenderPipeline};

/// Ply-specific extension data threaded through the render tree.
#[derive(Clone)]
pub struct PlyExt {
    /// Slot for the columns builder to write the left sidebar's block ID.
    /// Read by the title bar on the next frame to wire the toggle button.
    pub left_sidebar_block_id: Arc<Mutex<Option<String>>>,
}

/// Ply-specific render context wrapping the shared RenderContext with Ply extensions.
#[derive(Clone)]
pub struct RenderContext {
    pub ctx: holon_frontend::RenderContext,
    pub ext: PlyExt,
}

impl std::ops::Deref for RenderContext {
    type Target = holon_frontend::RenderContext;
    fn deref(&self) -> &Self::Target {
        &self.ctx
    }
}

impl RenderContext {
    pub fn with_row(&self, row: DataRow) -> Self {
        Self {
            ctx: self.ctx.with_row(row),
            ext: self.ext.clone(),
        }
    }

    pub fn with_data_rows(&self, data_rows: Vec<DataRow>) -> Self {
        Self {
            ctx: self.ctx.with_data_rows(data_rows),
            ext: self.ext.clone(),
        }
    }

    pub fn deeper_query(&self) -> Self {
        Self {
            ctx: self.ctx.deeper_query(),
            ext: self.ext.clone(),
        }
    }
}

pub fn new_render_context(
    session: Arc<FrontendSession>,
    runtime_handle: tokio::runtime::Handle,
    block_watch: holon_frontend::BlockWatchRegistry,
    left_sidebar_block_id: Arc<Mutex<Option<String>>>,
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
        ext: PlyExt {
            left_sidebar_block_id,
        },
    }
}
