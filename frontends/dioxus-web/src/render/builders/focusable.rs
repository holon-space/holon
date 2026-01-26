use super::prelude::*;

pub fn render(child: &Box<ViewModel>, _ctx: &DioxusRenderContext) -> Element {
    rsx! { RenderNode { node: (**child).clone() } }
}
