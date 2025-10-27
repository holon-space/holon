use super::prelude::*;

holon_macros::widget_builder! {
    raw fn focusable(ba: BA<'_>) -> ViewModel {
        let child = if let Some(child_expr) = ba.args.positional_exprs.first() {
            (ba.interpret)(child_expr, ba.ctx)
        } else {
            ViewModel::empty()
        };
        ViewModel {
            children: vec![Arc::new(child)],
            ..ViewModel::from_widget("focusable", std::collections::HashMap::new())
        }
    }
}
