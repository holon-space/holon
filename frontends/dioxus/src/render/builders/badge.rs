use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    let label = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("label"))
        .unwrap_or("")
        .to_string();

    rsx! {
        span {
            font_size: "11px",
            color: "var(--text-muted)",
            background_color: "var(--surface-elevated)",
            padding: "2px 8px",
            border_radius: "12px",
            {label}
        }
    }
}
