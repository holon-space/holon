use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    let name = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("name"))
        .unwrap_or("?")
        .to_string();
    AnyView::new(text(name).size(16.0))
}
