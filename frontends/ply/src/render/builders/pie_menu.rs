use super::prelude::*;

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let child = if let Some(child_expr) = args.positional_exprs.first() {
        interpret(child_expr, ctx)
    } else {
        empty_widget()
    };

    // Pie menu not yet supported — render child only
    child
}
