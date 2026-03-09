use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let checked = ba.args.get_bool("checked").unwrap_or(false);
    let symbol = if checked { "[x]" } else { "[ ]" };
    let color = if checked {
        tc(&ba, |t| t.success)
    } else {
        tc(&ba, |t| t.text_secondary)
    };

    div().child(symbol).text_color(color)
}
