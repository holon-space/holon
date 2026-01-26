use std::sync::{Arc, Mutex};

use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_api::EntityUri;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::WidgetState;

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
    pub services: Arc<dyn BuilderServices>,
    pub ext: PlyExt,
}

impl std::ops::Deref for RenderContext {
    type Target = holon_frontend::RenderContext;
    fn deref(&self) -> &Self::Target {
        &self.ctx
    }
}

impl RenderContext {
    pub fn with_row(&self, row: Arc<DataRow>) -> Self {
        Self {
            ctx: self.ctx.with_row(row),
            services: self.services.clone(),
            ext: self.ext.clone(),
        }
    }

    pub fn with_data_rows(&self, data_rows: Vec<Arc<DataRow>>) -> Self {
        Self {
            ctx: self.ctx.with_data_rows(data_rows),
            services: self.services.clone(),
            ext: self.ext.clone(),
        }
    }

    pub fn deeper_query(&self) -> Self {
        Self {
            ctx: self.ctx.deeper_query(),
            services: self.services.clone(),
            ext: self.ext.clone(),
        }
    }

    /// Get block data (render expr + rows) via services, ensuring a watcher is running.
    pub fn get_block_data(&self, id: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        self.services.get_block_data(id)
    }

    /// Look up widget state by block ID.
    pub fn widget_state(&self, id: &str) -> WidgetState {
        self.services.widget_state(id)
    }

    /// Dispatch an operation intent via services.
    pub fn dispatch_intent(&self, intent: holon_frontend::operations::OperationIntent) {
        self.services.dispatch_intent(intent);
    }
}

pub fn new_render_context(
    services: Arc<dyn BuilderServices>,
    left_sidebar_block_id: Arc<Mutex<Option<String>>>,
) -> RenderContext {
    RenderContext {
        ctx: holon_frontend::RenderContext::default(),
        services,
        ext: PlyExt {
            left_sidebar_block_id,
        },
    }
}
