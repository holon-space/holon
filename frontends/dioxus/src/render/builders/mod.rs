mod prelude;
mod stub;

holon_macros::builder_registry!("src/render/builders",
    skip: [prelude, stub],
    register: Element
);

use dioxus::prelude::*;
use holon_frontend::render_interpreter::{BuilderArgs, RenderInterpreter};

pub(crate) type BA<'a> = BuilderArgs<'a, Element>;

pub fn create_interpreter() -> RenderInterpreter<Element> {
    let mut interp = RenderInterpreter::new();

    register_all(&mut interp);

    interp.register("source_editor", source_block::build);

    for name in [
        "block_operations",
        "pie_menu",
        "drop_zone",
        "query_result",
        "draggable",
    ] {
        interp.register(name, stub::build);
    }

    interp.set_annotator(|element, _builder_name, ctx| {
        if let Some(id) = ctx.row().get("id").and_then(|v| v.as_string()) {
            let id = id.to_string();
            rsx! { div { id: "{id}", {element} } }
        } else {
            element
        }
    });

    interp
}
