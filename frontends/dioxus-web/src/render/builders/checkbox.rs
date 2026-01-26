use super::prelude::*;

pub fn render(checked: &bool, _ctx: &DioxusRenderContext) -> Element {
    let checked = *checked;
    rsx! { input { r#type: "checkbox", checked, disabled: true } }
}
