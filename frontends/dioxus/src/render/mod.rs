pub mod builders;

use std::sync::Arc;

use dioxus::prelude::*;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::{FrontendSession, RenderContext};

pub fn render_snapshot(
    render_expr: &RenderExpr,
    data_rows: &[Arc<DataRow>],
    session: &Arc<FrontendSession>,
    rt: &tokio::runtime::Handle,
) -> Element {
    let interp = builders::create_interpreter();
    let ctx = RenderContext::new(Arc::clone(session), rt.clone());
    let render_ctx = ctx.with_data_rows(data_rows.to_vec());
    interp.interpret(render_expr, &render_ctx)
}
