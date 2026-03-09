use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    let checked = ba.args.get_bool("checked").unwrap_or(false);
    let (symbol, color) = if checked {
        ("[x]", "var(--success)")
    } else {
        ("[ ]", "var(--text-muted)")
    };
    rsx! { span { font_size: "14px", color: color, {symbol} } }
}
