pub mod builders;

use std::sync::Arc;

use dioxus::prelude::*;
use holon_api::widget_spec::WidgetSpec;
use holon_frontend::{FrontendSession, RenderContext};

pub fn render_widget_spec(
    widget_spec: &WidgetSpec,
    session: &Arc<FrontendSession>,
    rt: &tokio::runtime::Handle,
    is_screen_layout: bool,
) -> Element {
    let interp = builders::create_interpreter();
    let mut ctx = RenderContext::new(Arc::clone(session), rt.clone());
    ctx.is_screen_layout = is_screen_layout;
    let data_rows: Vec<_> = widget_spec.data.iter().map(|r| r.data.clone()).collect();
    let render_ctx = ctx.with_data_rows(data_rows);
    interp.interpret(&widget_spec.render_expr, &render_ctx)
}
