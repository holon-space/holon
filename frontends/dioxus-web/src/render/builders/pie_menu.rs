use super::prelude::*;

pub fn render(_fields: &String, child: &Box<ViewModel>, _ctx: &DioxusRenderContext) -> Element {
    rsx! { RenderNode { node: (**child).clone() } }
}
