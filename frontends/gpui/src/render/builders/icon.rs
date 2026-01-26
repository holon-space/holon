use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let name = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("name"))
        .unwrap_or("circle")
        .to_string();

    div().child(format!("[{name}]"))
}
