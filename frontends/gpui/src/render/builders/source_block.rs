use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;

use super::operation_helpers::{get_entity_name, get_row_id};

pub fn build(ba: BA<'_>) -> Div {
    let language = ba.args.get_string("language").unwrap_or("text").to_string();
    let source = ba
        .args
        .get_string("source")
        .or_else(|| ba.args.get_string("content"))
        .unwrap_or("")
        .to_string();
    let name = ba.args.get_string("name").unwrap_or("").to_string();

    let mut container = div().flex_col().gap_1();

    let mut header = div().flex().flex_row().gap_2();
    header = header.child(
        div()
            .text_xs()
            .text_color(tc(&ba, |t| t.primary_light))
            .child(language.clone()),
    );
    if !name.is_empty() {
        header = header.child(
            div()
                .text_xs()
                .text_color(tc(&ba, |t| t.text_secondary))
                .child(name),
        );
    }

    let exec_ops: Vec<_> = ba
        .ctx
        .operations
        .iter()
        .filter(|ow| ow.descriptor.name == "execute_source_block")
        .collect();
    if let Some(exec_op) = exec_ops.first() {
        let row_id = get_row_id(ba.ctx);
        let entity_name =
            get_entity_name(ba.ctx).unwrap_or_else(|| exec_op.descriptor.entity_name.to_string());
        let op_name = exec_op.descriptor.name.clone();
        let session = ba.ctx.session.clone();
        let handle = ba.ctx.runtime_handle.clone();
        header = header.child(
            div()
                .text_xs()
                .text_color(tc(&ba, |t| t.success))
                .cursor_pointer()
                .child("[run]")
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
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

    container.child(
        div()
            .rounded(px(4.0))
            .bg(tc(&ba, |t| t.background_secondary))
            .overflow_hidden()
            .p_2()
            .text_xs()
            .child(source),
    )
}
