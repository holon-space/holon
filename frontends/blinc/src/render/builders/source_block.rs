use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::prelude::*;
use holon_api::Value;

use super::operation_helpers::{get_entity_name, get_row_id};

/// source_block(language:"holon_prql", source:"...", name:"...", editable:true) — collapsible source display.
pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let theme = ThemeState::get();

    let language = args.get_string("language").unwrap_or("text").to_string();
    let source = args
        .get_string("source")
        .or_else(|| args.get_string("content"))
        .unwrap_or("")
        .to_string();
    let name = args.get_string("name").unwrap_or("").to_string();
    let editable = args.get_bool("editable").unwrap_or(false);

    let mut container = div().flex_col().gap(4.0);

    // Header: language badge + name
    let mut header = div().flex_row().gap(8.0);
    header = header.child(
        text(language)
            .size(11.0)
            .color(theme.color(ColorToken::Accent)),
    );
    if !name.is_empty() {
        header = header.child(
            text(name)
                .size(11.0)
                .color(theme.color(ColorToken::TextSecondary)),
        );
    }

    // Execute button
    let exec_ops: Vec<_> = ctx
        .operations
        .iter()
        .filter(|ow| ow.descriptor.name == "execute_source_block")
        .collect();
    if let Some(exec_op) = exec_ops.first() {
        let row_id = get_row_id(ctx);
        let entity_name =
            get_entity_name(ctx).unwrap_or_else(|| exec_op.descriptor.entity_name.to_string());
        let op_name = exec_op.descriptor.name.clone();
        let session = ctx.session.clone();
        let handle = ctx.runtime_handle.clone();
        header = header.child(
            div()
                .child(
                    text("[run]".to_string())
                        .size(11.0)
                        .color(theme.color(ColorToken::Success)),
                )
                .on_click(move |_| {
                    let Some(ref id) = row_id else { return };
                    let mut params = HashMap::new();
                    params.insert("id".to_string(), Value::String(id.clone()));
                    holon_frontend::operations::dispatch_operation(
                        &handle,
                        &session,
                        entity_name.clone(),
                        op_name.clone(),
                        params,
                    );
                }),
        );
    }
    container = container.child(header);

    // Source content
    if editable && super::operation_helpers::find_set_field_op("source", &ctx.operations).is_some()
    {
        let state = TextAreaState::with_value(&source);
        let shared: SharedTextAreaState = Arc::new(Mutex::new(state));

        let op = super::operation_helpers::find_set_field_op("source", &ctx.operations);
        let row_id = get_row_id(ctx);

        let widget = match (op, row_id) {
            (Some(op), Some(row_id)) => {
                let entity_name =
                    get_entity_name(ctx).unwrap_or_else(|| op.entity_name.to_string());
                let op_name = op.name.clone();
                let session = ctx.session.clone();
                let handle = ctx.runtime_handle.clone();
                let shared_for_blur = shared.clone();
                let last_dispatched: Arc<Mutex<String>> = Arc::new(Mutex::new(source.clone()));

                text_area(&shared).font_size(13.0).on_blur(move |_| {
                    let new_value = shared_for_blur.lock().unwrap().value();
                    let mut last = last_dispatched.lock().unwrap();
                    if *last != new_value {
                        *last = new_value.clone();
                        let mut params = HashMap::new();
                        params.insert("id".into(), Value::String(row_id.clone()));
                        params.insert("field".into(), Value::String("source".into()));
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
            _ => text_area(&shared).font_size(13.0),
        };

        container.child(widget)
    } else {
        container.child(
            text(source)
                .size(13.0)
                .color(theme.color(ColorToken::TextPrimary)),
        )
    }
}
