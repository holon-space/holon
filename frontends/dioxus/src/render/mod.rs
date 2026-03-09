pub mod builders;

use std::sync::Arc;

use dioxus::prelude::*;
use holon_api::widget_spec::WidgetSpec;
use holon_frontend::{FrontendSession, RenderContext};

pub fn render_widget_spec(
    widget_spec: &WidgetSpec,
    session: &Arc<FrontendSession>,
    rt: &tokio::runtime::Handle,
) -> Element {
    let interp = builders::create_interpreter();
    let ctx = RenderContext::new(Arc::clone(session), rt.clone());
    let render_ctx = ctx.with_data_rows(widget_spec.data.clone());
    interp.interpret(&widget_spec.render_expr, &render_ctx)
}
