use super::prelude::*;
use holon_api::EntityUri;

pub fn render(
    _entity_uri: &EntityUri,
    _modes: &String,
    child: &Box<ViewModel>,
    _ctx: &DioxusRenderContext,
) -> Element {
    rsx! { RenderNode { node: (**child).clone() } }
}
