use super::prelude::*;
use holon_frontend::ViewModel;

// TODO: wire focus tracking via GPUI FocusHandle
pub fn render(child: &Box<ViewModel>, ctx: &GpuiRenderContext) -> AnyElement {
    super::render(child, ctx)
}
