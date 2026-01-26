use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    let h = ba
        .args
        .get_f64("height")
        .or(ba.args.get_f64("h"))
        .unwrap_or(0.0);
    if h > 0.0 {
        let height = format!("{}px", h);
        rsx! { div { height: "{height}" } }
    } else {
        rsx! { div { flex: "1" } }
    }
}
