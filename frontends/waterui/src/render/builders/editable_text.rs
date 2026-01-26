use std::cell::RefCell;
use std::rc::Rc;

use waterui::reactive::binding;

use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
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

    if op.is_none() {
        return AnyView::new(text(content).size(14.0));
    }

    let source_binding: Binding<Str> = binding(Str::from(content.clone()));

    match (op, row_id) {
        (Some(op), Some(row_id)) => {
            let entity_name = holon_frontend::operations::get_entity_name(ba.ctx)
                .unwrap_or_else(|| op.entity_name.to_string());
            let op_name = op.name.clone();
            let session = ba.ctx.session.clone();
            let handle = ba.ctx.runtime_handle.clone();
            let last_dispatched: Rc<RefCell<String>> = Rc::new(RefCell::new(content));

            let mapped: Binding<Str> = Binding::mapping(
                &source_binding,
                |v| v,
                move |source, new_val: Str| {
                    source.set(new_val.clone());
                    let new_string: String = new_val.into();
                    let mut last = last_dispatched.borrow_mut();
                    if *last != new_string {
                        *last = new_string.clone();
                        let mut params = HashMap::new();
                        params.insert("id".into(), Value::String(row_id.clone()));
                        params.insert("field".into(), Value::String(field.clone()));
                        params.insert("value".into(), Value::String(new_string));
                        holon_frontend::operations::dispatch_operation(
                            &handle,
                            &session,
                            entity_name.clone(),
                            op_name.clone(),
                            params,
                        );
                    }
                },
            );

            AnyView::new(TextField::new(&mapped))
        }
        _ => AnyView::new(TextField::new(&source_binding)),
    }
}
