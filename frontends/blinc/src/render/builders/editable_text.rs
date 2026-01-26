use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::prelude::*;
use holon_api::Value;

use super::operation_helpers::{get_entity_name, get_row_id};

/// editable_text(col("field")) — in-place text editing.
///
/// Renders a text area bound to the field value. When a write operation and row id
/// are available, dispatches `set_field` on blur to persist changes.
pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let theme = ThemeState::get();

    let content = args
        .get_positional_string(0)
        .or_else(|| args.get_string("content"))
        .unwrap_or("")
        .to_string();

    let field = args
        .get_positional_column_name(0)
        .unwrap_or("content")
        .to_string();

    let has_write_op =
        super::operation_helpers::find_set_field_op(&field, &ctx.operations).is_some();

    if !has_write_op {
        return div().child(
            text(content)
                .size(14.0)
                .color(theme.color(ColorToken::TextPrimary)),
        );
    }

    let state = TextAreaState::with_value(&content);
    let shared: SharedTextAreaState = Arc::new(Mutex::new(state));

    // Wire dispatch only when we have the operation details and a row id
    let op = super::operation_helpers::find_set_field_op(&field, &ctx.operations);
    let row_id = get_row_id(ctx);

    let widget = match (op, row_id) {
        (Some(op), Some(row_id)) => {
            let entity_name = get_entity_name(ctx).unwrap_or_else(|| op.entity_name.to_string());
            let op_name = op.name.clone();
            let session = ctx.session().clone();
            let handle = ctx.runtime_handle().clone();
            let shared_for_blur = shared.clone();
            let last_dispatched: Arc<Mutex<String>> = Arc::new(Mutex::new(content.clone()));

            text_area(&shared).font_size(14.0).on_blur(move |_| {
                let new_value = shared_for_blur.lock().unwrap().value();
                let mut last = last_dispatched.lock().unwrap();
                if *last != new_value {
                    *last = new_value.clone();
                    let mut params = HashMap::new();
                    params.insert("id".into(), Value::String(row_id.clone()));
                    params.insert("field".into(), Value::String(field.clone()));
                    params.insert("value".into(), Value::String(new_value));
                    holon_frontend::operations::dispatch_operation(
                        &handle,
                        &session,
                        entity_name.clone(),
                        op_name.clone(),
                        params,
                    );
                }
            })
        }
        _ => text_area(&shared).font_size(14.0),
    };

    div().child(widget)
}
