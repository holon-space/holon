use super::prelude::*;

holon_macros::widget_builder! {
    raw fn block_operations(ba: BA<'_>) -> ViewModel {
        let operations: String = ba
            .ctx
            .operations
            .iter()
            .map(|ow| ow.descriptor.name.clone())
            .collect::<Vec<_>>()
            .join(",");

        let mut __props = std::collections::HashMap::new();
        __props.insert("operations".to_string(), Value::String(operations));
        ViewModel::from_widget("block_operations", __props)
    }
}
