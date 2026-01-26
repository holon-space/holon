use std::sync::Arc;

use blinc_core::State;
use holon_frontend::FrontendSession;

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

pub type RenderContext = holon_frontend::RenderContext<BlincExt>;

pub fn new_render_context(
    session: Arc<FrontendSession>,
    runtime_handle: tokio::runtime::Handle,
    block_cache: holon_frontend::BlockRenderCache,
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
        ext: BlincExt::default(),
        block_cache,
        widget_states,
    }
}
