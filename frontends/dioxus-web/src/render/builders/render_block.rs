use super::prelude::*;

pub fn render(content: &Box<ViewModel>, _: &DioxusRenderContext) -> Element {
    rsx! { RenderNode { node: (**content).clone() } }
}
