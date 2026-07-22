use super::prelude::*;

holon_macros::widget_builder! {
    fn row(#[default = 8.0] gap: f32, #[default = "center"] align: String, children: Collection) {
        let mut __props = std::collections::HashMap::new();
        __props.insert("gap".to_string(), Value::Float(gap as f64));
        __props.insert("align".to_string(), Value::String(align));
        ViewModel {
            children: children.into_static_items().into_iter().map(Arc::new).collect(),
            ..ViewModel::from_widget("row", __props)
        }
    }
}
