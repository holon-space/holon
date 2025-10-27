use super::prelude::*;

holon_macros::widget_builder! {
    raw fn card(ba: BA<'_>) -> ViewModel {
        // Named "accent" takes priority — positional[0] is polluted by eval_to_value
        // flattening the first child expr's text content.
        let accent = ba.args.get_string("accent")
            .map(|s| s.to_string())
            .or_else(|| ba.args.get_positional_string(0))
            .unwrap_or_default();

        let children: Vec<ViewModel> = if ba.args.positional_exprs.is_empty() {
            vec![]
        } else {
            ba.args.positional_exprs.iter()
                .map(|expr| (ba.interpret)(expr, ba.ctx))
                .collect()
        };

        let mut __props = std::collections::HashMap::new();
        __props.insert("accent".to_string(), Value::String(accent));
        ViewModel {
            children: children.into_iter().map(Arc::new).collect(),
            ..ViewModel::from_widget("card", __props)
        }
    }
}
