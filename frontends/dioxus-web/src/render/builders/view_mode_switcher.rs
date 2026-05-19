use super::prelude::*;
use holon_api::EntityUri;

pub fn render(
    _: &EntityUri,
    _: &String,
    child: &Box<ViewModel>,
    _: &DioxusRenderContext,
) -> Element {
    rsx! { RenderNode { node: (**child).clone() } }
}
