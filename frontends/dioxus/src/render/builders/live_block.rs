use super::prelude::*;
use holon_frontend::render_interpreter::{shared_live_block_build, BuilderArgs};

pub fn build(ba: BA<'_>) -> Element {
    let block_id = match ba.ctx.row().get("id").and_then(|v| v.as_string()) {
        Some(id) => id.to_string(),
        None => {
            return rsx! { span { font_size: "12px", color: "var(--error)", "[live_block: no id in row]" } }
        }
    };
    let ref_args =
        holon_api::render_eval::ResolvedArgs::from_positional_value(Value::String(block_id));
    let ref_ba = BuilderArgs {
        args: &ref_args,
        ctx: ba.ctx,
        interpret: ba.interpret,
    };
    match shared_live_block_build(&ref_ba) {
        Ok(widget) => widget,
        Err(msg) => rsx! { span { font_size: "12px", color: "var(--error)", {msg} } },
    }
}
