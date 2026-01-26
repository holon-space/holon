use std::collections::HashMap;

use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    let content = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("content"))
        .unwrap_or("")
        .to_string();

    let field = ba
        .args
        .get_string("_pos_0_field")
        .unwrap_or("content")
        .to_string();

    let op = holon_frontend::operations::find_set_field_op(&field, &ba.ctx.operations);
    let row_id = holon_frontend::operations::get_row_id(ba.ctx);

    match (op, row_id) {
        (Some(op), Some(row_id)) => {
            let entity_name = holon_frontend::operations::get_entity_name(ba.ctx)
                .unwrap_or_else(|| op.entity_name.to_string());
            let op_name = op.name.clone();
            let session = ba.ctx.session.clone();
            let last_dispatched = content.clone();

            rsx! {
                input {
                    r#type: "text",
                    value: "{content}",
                    style: "font-size: 14px; background: transparent; color: inherit; border: 1px solid var(--border); padding: 2px 4px; width: 100%; outline: none;",
                    onchange: move |evt: Event<FormData>| {
                        let new_value = evt.value();
                        if new_value != last_dispatched {
                            let mut params = HashMap::new();
                            params.insert("id".into(), Value::String(row_id.clone()));
                            params.insert("field".into(), Value::String(field.clone()));
                            params.insert("value".into(), Value::String(new_value));
                            crate::operations::dispatch_operation(
                                &session,
                                entity_name.clone(),
                                op_name.clone(),
                                params,
                            );
                        }
                    },
                }
            }
        }
        _ => {
            rsx! { span { font_size: "14px", {content} } }
        }
    }
}
