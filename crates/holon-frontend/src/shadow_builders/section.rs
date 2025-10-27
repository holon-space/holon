use super::prelude::*;

holon_macros::widget_builder! {
    fn section(#[default = "Section"] title: String, children: Collection) {
        let mut __props = std::collections::HashMap::new();
        __props.insert("title".to_string(), Value::String(title));
        ViewModel {
            children: children.into_static_items().into_iter().map(Arc::new).collect(),
            ..ViewModel::from_widget("section", __props)
        }
    }
}
