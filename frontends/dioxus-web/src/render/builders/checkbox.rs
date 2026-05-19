use super::prelude::*;

pub fn render(checked: &bool, _: &DioxusRenderContext) -> Element {
    let checked = *checked;
    rsx! { input { r#type: "checkbox", checked, disabled: true } }
}
