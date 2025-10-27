use super::prelude::*;

holon_macros::widget_builder! {
    raw fn collapsible(ba: BA<'_>) -> ViewModel {
        let header = ba.args.get_string("summary")
            .map(|s| s.to_string())
            .unwrap_or_default();
        let icon = ba.args.get_string("icon")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "▸".to_string());

        let children: Vec<ViewModel> = if ba.args.positional_exprs.is_empty() {
            vec![]
        } else {
            ba.args.positional_exprs.iter()
                .map(|expr| (ba.interpret)(expr, ba.ctx))
                .collect()
        };

        let mut __props = std::collections::HashMap::new();
        __props.insert("header".to_string(), Value::String(header));
        __props.insert("icon".to_string(), Value::String(icon));
        ViewModel {
            children: children.into_iter().map(Arc::new).collect(),
            ..ViewModel::from_widget("collapsible", __props)
        }
    }
}
