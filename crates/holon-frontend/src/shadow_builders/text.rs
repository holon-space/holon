use futures_signals::signal::SignalExt;

use super::prelude::*;
use crate::reactive_view_model::DropTask;

holon_macros::widget_builder! {
    fn text(content: String, #[default = false] bold: bool, #[default = 14.0] size: f32, color: Option<String>, style: Option<String>, #[default = false] truncate: bool) {
        // When positional 0 is a `col("foo")` ref, capture the field name so we
        // can re-derive `content` on every CDC write to the row. The macro's
        // auto-extracted `content: String` is just the snapshot at build time;
        // without a subscription it would freeze and `text(col("content"))`
        // would render stale text after split/join/external edit. Static
        // `text("label")` callers leave `field` as `None` and skip the
        // subscription — nothing to track.
        let field = ba.args.get_positional_column_name(0).map(|s| s.to_string());

        // A semantic `style` keyword (e.g. `#{style: "h1"}`, used by the
        // page-title variant `text(col("content"), #{style: "h1"})`) is carried
        // through as a prop and resolved into the heading type scale at render
        // time (see the gpui `text` builder). Before `style` was a declared
        // param the kwarg was silently dropped and the title rendered at body
        // size. Validate the keyword loudly here at the build boundary — an
        // unrecognized style is a config error, not a silent body-size default.
        // Resolution itself lives at render (not here) so the fast-path
        // `resolve_props_from_args` recompute and this full build agree.
        if let Some(kw) = style.as_deref() {
            if holon_api::render_eval::text_style_font_size(kw).is_none() {
                tracing::warn!(
                    "text(): unknown style keyword {kw:?} — will render at body size; add it to \
                     holon_api::render_eval::text_style_font_size"
                );
            }
        }

        let mut __props = std::collections::HashMap::new();
        __props.insert("content".to_string(), Value::String(content));
        __props.insert("bold".to_string(), Value::Boolean(bold));
        __props.insert("size".to_string(), Value::Float(size as f64));
        // `#{truncate: true}` = "this label yields when its row is short of
        // room" — it may shrink below its content width and clip, with an
        // ellipsis. Declared per call site rather than inferred: a label in a
        // plain `row` shares its width with siblings that must not be pushed
        // out, while a `table` cell is already inside a column box that sizes
        // it, and making that one nowrap silently retires its wrapping.
        __props.insert("truncate".to_string(), Value::Boolean(truncate));
        if let Some(ref s) = style {
            __props.insert("style".to_string(), Value::String(s.clone()));
        }
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
                if let Some(new_content) = super::prelude::content_from_row(&row, &field) {
                    props_handle
                        .lock_mut()
                        .insert("content".to_string(), Value::String(new_content));
                }
                async {}
            }));
            vm.subscriptions.push(DropTask::new(task));
        }

        vm
    }
}
