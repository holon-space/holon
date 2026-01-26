mod prelude;

holon_macros::builder_registry!("src/render/builders",
    skip: [prelude],
    register: AnyView
);

use holon_frontend::render_interpreter::{
    shared_col_build, shared_live_query_build, shared_render_entity_build, BuilderArgs,
    RenderBlockResult, RenderInterpreter,
};
use waterui::prelude::*;

pub(crate) type BA<'a> = BuilderArgs<'a, AnyView>;

pub fn create_interpreter() -> RenderInterpreter<AnyView> {
    let mut interp = RenderInterpreter::new();

    register_all(&mut interp);

    interp.register("source_editor", source_block::build);

    interp.register("column", |ba: BA| {
        let children = shared_col_build(&ba);
        AnyView::new(vstack(children))
    });
    interp.register("live_block", |ba: BA| {
        use holon_frontend::render_interpreter::shared_live_block_build;
        match shared_live_block_build(&ba) {
            Ok(w) => w,
            Err(msg) => AnyView::new(text(msg).size(12.0).foreground(Color::srgb_hex("#FF0000"))),
        }
    });
    interp.register("live_query", |ba: BA| match shared_live_query_build(&ba) {
        Ok(result) => result.content,
        Err(msg) => AnyView::new(text(msg).size(12.0).foreground(Color::srgb_hex("#FF0000"))),
    });
    interp.register("render_entity", |ba: BA| {
        match shared_render_entity_build(&ba) {
            RenderBlockResult::ProfileWidget { render, operations } => {
                let ctx = ba.ctx.with_operations(operations);
                (ba.interpret)(&render, &ctx)
            }
            RenderBlockResult::Empty => AnyView::new(()),
            RenderBlockResult::Error(msg) => {
                AnyView::new(text(msg).size(12.0).foreground(Color::srgb_hex("#FF0000")))
            }
        }
    });

    for name in [
        "badge",
        "block_operations",
        "pie_menu",
        "state_toggle",
        "focusable",
        "drop_zone",
        "query_result",
        "draggable",
    ] {
        interp.register(name, |ba: BA| {
            if let Some(tmpl) = ba
                .args
                .get_template("item_template")
                .or(ba.args.get_template("item"))
            {
                let views: Vec<AnyView> = if ba.ctx.data_rows.is_empty() {
                    vec![(ba.interpret)(tmpl, ba.ctx)]
                } else {
                    ba.ctx
                        .data_rows
                        .iter()
                        .map(|row| (ba.interpret)(tmpl, &ba.ctx.with_row(row.clone())))
                        .collect()
                };
                AnyView::new(vstack(views))
            } else {
                AnyView::new(
                    text("[stub]")
                        .size(12.0)
                        .foreground(Color::srgb_hex("#808080")),
                )
            }
        });
    }

    // WaterUI: no geometry API, but annotate for future use.
    // AnyView is opaque — we can't attach IDs without framework support.

    interp
}
