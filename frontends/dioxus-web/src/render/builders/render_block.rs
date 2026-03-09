use super::prelude::*;

pub fn render(content: &Box<ViewModel>, _ctx: &DioxusRenderContext) -> Element {
    rsx! { RenderNode { node: (**content).clone() } }
}
