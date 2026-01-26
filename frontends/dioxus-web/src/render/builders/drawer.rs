use super::prelude::*;

pub fn render(
    _block_id: &String,
    child: &Box<ViewModel>,
    _ctx: &DioxusRenderContext,
) -> Element {
    rsx! { RenderNode { node: (**child).clone() } }
}
