use super::prelude::*;
use crate::reactive_view_model::DropTask;
use futures_signals::signal::SignalExt;

holon_macros::widget_builder! {
    fn text(content: String, #[default = false] bold: bool, #[default = 14.0] size: f32, color: Option<String>) {
        // When positional 0 is a `col("foo")` ref, capture the field name so we
        // can re-derive `content` on every CDC write to the row. The macro's
        // auto-extracted `content: String` is just the snapshot at build time;
        // without a subscription it would freeze and `text(col("content"))`
        // would render stale text after split/join/external edit. Static
        // `text("label")` callers leave `field` as `None` and skip the
        // subscription — nothing to track.
        let field = ba.args.get_positional_column_name(0).map(|s| s.to_string());

        let mut __props = std::collections::HashMap::new();
        __props.insert("content".to_string(), Value::String(content));
        __props.insert("bold".to_string(), Value::Boolean(bold));
        __props.insert("size".to_string(), Value::Float(size as f64));
        if let Some(c) = color {
            __props.insert("color".to_string(), Value::String(c));
        }
        // Record the bound column so the gpui builder can scope geometry
        // tracking. `inv-displayed-text` only compares against
        // `block.content_text()`, so widgets reading other columns
        // (e.g. `text(col("name"))` in the left sidebar) shouldn't be
        // tracked — their displayed string is correct but compares wrong.
        if let Some(ref f) = field {
            __props.insert("field".to_string(), Value::String(f.clone()));
        }

        // Only share the per-row data handle (and subscribe) when the
        // first arg is a `col(...)` ref — i.e. there's a real row binding.
        // `text("Journals")` and other static labels keep the default empty
        // data row so `row_id()` returns None and `inv-displayed-text`
        // doesn't try to compare them against a non-existent SQL block.
        let Some(field) = field else {
            return ViewModel::from_widget("text", __props);
        };

        let data = ba.ctx.data_mutable();
        let mut vm = ViewModel {
            data: data.clone(),
            ..ViewModel::from_widget("text", __props)
        };

        if let Some(runtime) = ba.services.try_runtime_handle() {
            let props_handle = vm.props.clone();
            let task = runtime.spawn(data.signal_cloned().for_each(move |row| {
                let new_content = row
                    .get(&field)
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string();
                props_handle
                    .lock_mut()
                    .insert("content".to_string(), Value::String(new_content));
                async {}
            }));
            vm.subscriptions.push(DropTask::new(task));
        }

        vm
    }
}
