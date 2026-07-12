use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Icon { name, .. } = &node.kind else {
        return rsx! {};
    };
    // The literal "·{name}" text stays in the DOM (tooling / a11y); CSS
    // (`.holon-icon`) renders it as a bullet and reveals the label on row
    // hover. `title` gives an immediate tooltip.
    rsx! { span { class: "holon-icon", title: "{name}", "·{name}" } }
}
