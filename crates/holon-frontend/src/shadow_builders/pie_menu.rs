use super::prelude::*;

holon_macros::widget_builder! {
    raw fn pie_menu(ba: BA<'_>) -> ViewModel {
        let fields = ba.args.get_string("fields").unwrap_or("").to_string();
        let child = if let Some(child_expr) = ba.args.positional_exprs.first() {
            (ba.interpret)(child_expr, ba.ctx)
        } else {
            ViewModel::empty()
        };
        let mut __props = std::collections::HashMap::new();
        __props.insert("fields".to_string(), Value::String(fields));
        ViewModel {
            children: vec![Arc::new(child)],
            ..ViewModel::from_widget("pie_menu", __props)
        }
    }
}
