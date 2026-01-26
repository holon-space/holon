use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    let language = ba.args.get_string("language").unwrap_or("text").to_string();
    let source = ba
        .args
        .get_string("source")
        .or_else(|| ba.args.get_string("content"))
        .unwrap_or("")
        .to_string();
    let name = ba.args.get_string("name").unwrap_or("").to_string();

    rsx! {
        div { display: "flex", flex_direction: "column", gap: "4px",
            div { display: "flex", flex_direction: "row", gap: "8px", align_items: "center",
                span { font_size: "11px", color: "var(--accent)", {language} }
                if !name.is_empty() {
                    span { font_size: "11px", color: "var(--text-muted)", {name} }
                }
            }
            pre {
                font_size: "13px",
                padding: "8px",
                background_color: "var(--surface-elevated)",
                border_radius: "4px",
                overflow_x: "auto",
                white_space: "pre-wrap",
                margin: "0",
                {source}
            }
        }
    }
}
