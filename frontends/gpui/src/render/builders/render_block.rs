use super::prelude::*;
use holon_frontend::ViewModel;

pub fn render(node: &ViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    use holon_frontend::view_model::NodeKind;
    let NodeKind::RenderBlock { content } = &node.kind else {
        unreachable!()
    };
    super::render(content, ctx)
}
