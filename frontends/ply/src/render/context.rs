use std::sync::{Arc, Mutex};

use holon_frontend::FrontendSession;

/// Ply-specific extension data threaded through the render tree.
#[derive(Clone)]
pub struct PlyExt {
    /// Slot for the columns builder to write the left sidebar's block ID.
    /// Read by the title bar on the next frame to wire the toggle button.
    pub left_sidebar_block_id: Arc<Mutex<Option<String>>>,
}

pub type RenderContext = holon_frontend::RenderContext<PlyExt>;

pub fn new_render_context(
    session: Arc<FrontendSession>,
    runtime_handle: tokio::runtime::Handle,
    block_cache: holon_frontend::BlockRenderCache,
    left_sidebar_block_id: Arc<Mutex<Option<String>>>,
) -> RenderContext {
    let widget_states = Arc::new(session.ui_settings().widgets);
    RenderContext {
        data_rows: Vec::new(),
        operations: Vec::new(),
        session,
        runtime_handle,
        depth: 0,
        query_depth: 0,
        is_screen_layout: false,
        ext: PlyExt {
            left_sidebar_block_id,
        },
        block_cache,
        widget_states,
    }
}
