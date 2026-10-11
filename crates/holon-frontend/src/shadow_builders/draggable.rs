// A drag source. `op` names the row operation the drag dispatches when it is
// dropped. The source binds it from the row's offered operations, so a row that
// is not offered it binds none and cannot be dragged. `op` is stamped as a prop
// because `render_entity` may union further row operations onto this node.
use super::prelude::*;

holon_macros::widget_builder! {
    raw fn draggable(ba: BA<'_>) -> ViewModel {
        let child = if let Some(child_expr) = ba.args.positional_exprs.first() {
            (ba.interpret)(child_expr, ba.ctx)
        } else {
            ViewModel::empty()
        };
        let op = ba.args.get_string("op").unwrap_or("move_block");
        // `row_id()` must name the dragged row: the GPUI draggable builder and
        // the headless drop walker both match by it.
        ViewModel {
            children: vec![Arc::new(child)],
            data: futures_signals::signal::Mutable::new(ba.ctx.row_arc()).read_only(),
            operations: super::prelude::offered_operation(ba.ctx, op),
            ..ViewModel::from_widget(
                "draggable",
                std::collections::HashMap::from([("op".to_string(), Value::String(op.to_string()))]),
            )
        }
    }
}
