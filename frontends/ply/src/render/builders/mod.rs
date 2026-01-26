pub(crate) mod operation_helpers;
mod prelude;

holon_macros::builder_registry!("src/render/builders",
    skip: [prelude, operation_helpers],
    dispatch: PlyWidget
);

use holon_api::render_eval::ResolvedArgs;

use super::context::RenderContext;
use super::PlyWidget;

/// Build a PlyWidget from a render function name and resolved arguments.
pub fn build(name: &str, args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    dispatch_build(name, args, ctx).unwrap_or_else(|| {
        tracing::warn!("Unknown builder: {name}");
        let name = name.to_string();
        Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
            ui.text(&format!("[unknown: {name}]"), |t| {
                t.font_size(12).color(0x888888u32)
            });
        })
    })
}
