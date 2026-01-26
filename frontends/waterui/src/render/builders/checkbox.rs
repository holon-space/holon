use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    let checked = ba.args.get_bool("checked").unwrap_or(false);
    let symbol = if checked { "\u{2611}" } else { "\u{2610}" };
    let color = if checked { "#4CAF50" } else { "#808080" };
    AnyView::new(text(symbol).size(14.0).foreground(Color::srgb_hex(color)))
}
