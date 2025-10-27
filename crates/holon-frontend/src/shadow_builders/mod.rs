mod prelude;

holon_macros::builder_registry!("src/shadow_builders",
    skip: [prelude],
    register: DisplayNode
);

use crate::display_node::DisplayNode;
use crate::render_interpreter::{shared_col_build, RenderInterpreter};

pub fn create_shadow_interpreter() -> RenderInterpreter<DisplayNode> {
    let mut interp = RenderInterpreter::new();
    register_all(&mut interp);
    interp.register("col", |ba: prelude::BA<'_>| {
        let children: Vec<DisplayNode> = shared_col_build(&ba);
        DisplayNode::layout("col", children)
    });
    interp.set_annotator(|mut node, _name, ctx| {
        node.operations = ctx.operations.clone();
        node
    });
    interp
}
