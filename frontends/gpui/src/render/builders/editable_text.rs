use super::prelude::*;

/// editable_text — renders as plain text for now.
/// GPUI text editing requires Entity<InputState> which needs a Window + Context,
/// not available in our stateless interpreter. Will be wired when we add
/// GPUI Entity-based rendering.
pub fn build(ba: BA<'_>) -> Div {
    let content = ba
        .args
        .get_positional_string(0)
        .or_else(|| ba.args.get_string("content"))
        .unwrap_or("")
        .to_string();

    div().child(content)
}
