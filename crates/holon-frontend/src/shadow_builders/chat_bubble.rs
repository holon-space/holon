use super::prelude::*;

holon_macros::widget_builder! {
    raw fn chat_bubble(ba: BA<'_>) -> ViewModel {
        // Named args first to avoid positional pollution from child exprs.
        let sender = ba.args.get_string("sender")
            .map(|s| s.to_string())
            .unwrap_or_default();
        let time = ba.args.get_string("time")
            .map(|s| s.to_string())
            .unwrap_or_default();

        let children: Vec<ViewModel> = if ba.args.positional_exprs.is_empty() {
            vec![]
        } else {
            ba.args.positional_exprs.iter()
                .map(|expr| (ba.interpret)(expr, ba.ctx))
                .collect()
        };

        let mut __props = std::collections::HashMap::new();
        __props.insert("sender".to_string(), Value::String(sender));
        __props.insert("time".to_string(), Value::String(time));
        ViewModel {
            children: children.into_iter().map(Arc::new).collect(),
            ..ViewModel::from_widget("chat_bubble", __props)
        }
    }
}
