use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    let name = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("name"))
        .unwrap_or("?")
        .to_string();
    rsx! { span { font_size: "16px", {name} } }
}
