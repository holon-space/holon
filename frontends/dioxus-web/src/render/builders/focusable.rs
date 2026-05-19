use super::prelude::*;

pub fn render(child: &Box<ViewModel>, _: &DioxusRenderContext) -> Element {
    rsx! { RenderNode { node: (**child).clone() } }
}
