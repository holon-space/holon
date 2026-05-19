use super::prelude::*;

pub fn render(
    _: &String,
    child: &Box<ViewModel>,
    _: &DioxusRenderContext,
) -> Element {
    rsx! { RenderNode { node: (**child).clone() } }
}
