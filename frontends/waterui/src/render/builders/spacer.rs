use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    let h = ba
        .args
        .get_f64("height")
        .or(ba.args.get_f64("h"))
        .unwrap_or(0.0) as f32;
    if h > 0.0 {
        AnyView::new(spacer().height(h))
    } else {
        AnyView::new(spacer())
    }
}
