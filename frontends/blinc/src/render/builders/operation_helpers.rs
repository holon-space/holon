use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use blinc_app::prelude::*;
use holon_api::Value;

use crate::render::context::RenderContext;

// Re-export from holon-frontend::operations — single source of truth.
pub use holon_frontend::operations::{
    find_ops_affecting, find_set_field_op, get_entity_name, get_row_id,
};

/// Build an editable text_area that dispatches a `set_field` operation on blur.
///
/// Returns `None` if no suitable operation or row ID is available, in which case
/// callers should fall back to a read-only display.
pub fn editable_source_widget(
    field: &str,
    initial_value: &str,
    ctx: &RenderContext,
) -> Option<TextArea> {
    let op = find_set_field_op(field, &ctx.operations)?;
    let row_id = get_row_id(ctx)?;

    let entity_name = get_entity_name(ctx).unwrap_or_else(|| op.entity_name.to_string());
    let op_name = op.name.clone();
    let session = ctx.session().clone();
    let handle = ctx.runtime_handle().clone();

    let state = TextAreaState::with_value(initial_value);
    let shared: SharedTextAreaState = Arc::new(Mutex::new(state));
    let shared_for_blur = shared.clone();
    let last_dispatched: Arc<Mutex<String>> = Arc::new(Mutex::new(initial_value.to_string()));
    let field = field.to_string();

    Some(text_area(&shared).font_size(13.0).on_blur(move |_| {
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
    }))
}
