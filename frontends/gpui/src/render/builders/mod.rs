pub(crate) mod operation_helpers;
mod prelude;

holon_macros::builder_registry!("src/render/builders",
    skip: [prelude, operation_helpers],
    register: Div,
    ext: crate::geometry::BoundsRegistry
);

use gpui::prelude::*;
use gpui::{div, Div};
use holon_frontend::render_interpreter::{BuilderArgs, RenderInterpreter};

use crate::geometry::BoundsRegistry;

pub(crate) type BA<'a> = BuilderArgs<'a, Div, BoundsRegistry>;

pub fn create_interpreter() -> RenderInterpreter<Div, BoundsRegistry> {
    let mut interp = RenderInterpreter::new();

    register_all(&mut interp);

    interp.register("col", |ba: BA<'_>| {
        let children: Vec<Div> = holon_frontend::render_interpreter::shared_col_build(&ba);
        let mut container = div().flex_col();
        for child in children {
            container = container.child(child);
        }
        container
    });

    // GPUI: `.id()` changes Div → Stateful<Div> (different type), so we can't
    // use the annotator for element IDs. Instead, GPUI uses the BoundsRegistry
    // approach: widgets record their bounds during render, keyed by entity ID
    // stored in the RenderContext. See `frontends/gpui/src/geometry.rs`.

    interp
}
