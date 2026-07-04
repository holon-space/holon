use futures_signals::signal::SignalExt;

use super::prelude::*;
use crate::reactive_view_model::DropTask;

holon_macros::widget_builder! {
    fn rendered_text(content: String, #[default = "content"] field: String) {
        let mut __props = std::collections::HashMap::new();
        __props.insert("content".to_string(), Value::String(content));
        __props.insert("field".to_string(), Value::String(field.clone()));

        let data = ba.ctx.data_mutable();
        let mut vm = ViewModel {
            data: data.clone(),
            operations: ba.ctx.operations.clone(),
            triggers: ba.ctx.triggers.clone(),
            ..ViewModel::from_widget("rendered_text", __props)
        };
        if let Some(runtime) = ba.services.try_runtime_handle() {
            let props_handle = vm.props.clone();
            let derive = move |row: Arc<holon_api::widget_spec::DataRow>| {
                if let Some(new_content) = super::prelude::content_from_row(&row, &field) {
                    props_handle
                        .lock_mut()
                        .insert("content".to_string(), Value::String(new_content));
                }
            };
            let task = runtime.spawn(data.signal_cloned().for_each(move |row| {
                derive(row);
                async {}
            }));
            vm.subscriptions.push(DropTask::new(task));
        }
        vm
    }
}
