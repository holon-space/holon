use super::prelude::*;

holon_macros::widget_builder! {
    raw fn draggable(ba: BA<'_>) -> ViewModel {
        let child = if let Some(child_expr) = ba.args.positional_exprs.first() {
            (ba.interpret)(child_expr, ba.ctx)
        } else {
            ViewModel::empty()
        };
        // Bind data to the current row so `row_id()` returns the block's id.
        // GPUI's draggable builder reads `node.row_id()` to set up `on_drag`
        // and the headless drop walker (and inv16) match by row_id; both
        // silently no-op without this binding.
        ViewModel {
            children: vec![Arc::new(child)],
            data: futures_signals::signal::Mutable::new(ba.ctx.row_arc()).read_only(),
            ..ViewModel::from_widget("draggable", std::collections::HashMap::new())
        }
    }
}
