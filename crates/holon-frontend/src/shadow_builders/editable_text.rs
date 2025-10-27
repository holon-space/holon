use super::prelude::*;
use crate::reactive_view_model::DropTask;
use futures_signals::signal::SignalExt;

holon_macros::widget_builder! {
    fn editable_text(content: String, #[default = "content"] field: String) {
        let mut __props = std::collections::HashMap::new();
        __props.insert("content".to_string(), Value::String(content));
        __props.insert("field".to_string(), Value::String(field.clone()));

        // Wire to the shared per-row signal cell. `data_mutable()` returns
        // a `ReadOnlyMutable` clone of the cell owned by `ReactiveRowSet`.
        // The subscription below re-derives `content` from the row's
        // `field`-named column on every CDC write — that's the
        // architectural fix for the post-edit / post-CDC stale-content
        // bug (see HANDOFF_TASK_STATE_TOGGLE.md "Other manifestations").
        let data = ba.ctx.data_mutable();
        let mut vm = ViewModel {
            data: data.clone(),
            operations: ba.ctx.operations.clone(),
            triggers: ba.ctx.triggers.clone(),
            ..ViewModel::from_widget("editable_text", __props)
        };
        if let Some(runtime) = ba.services.try_runtime_handle() {
            let props_handle = vm.props.clone();
            let derive = move |row: Arc<holon_api::widget_spec::DataRow>| {
                let new_content = row
                    .get(&field)
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string();
                props_handle
                    .lock_mut()
                    .insert("content".to_string(), Value::String(new_content));
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
