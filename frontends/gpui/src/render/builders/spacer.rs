use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let w = ba
        .args
        .get_f64("width")
        .or(ba.args.get_f64("w"))
        .unwrap_or(0.0) as f32;
    let h = ba
        .args
        .get_f64("height")
        .or(ba.args.get_f64("h"))
        .unwrap_or(0.0) as f32;

    let mut el = div();
    if w > 0.0 {
        el = el.w(px(w));
    }
    if h > 0.0 {
        el = el.h(px(h));
    }
    if w == 0.0 && h == 0.0 {
        el = el.flex_1();
    }
    el
}
