use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    let language = ba.args.get_string("language").unwrap_or("text").to_string();
    let source = ba
        .args
        .get_string("source")
        .or_else(|| ba.args.get_string("content"))
        .unwrap_or("")
        .to_string();
    let name = ba.args.get_string("name").unwrap_or("").to_string();

    let mut header_views: Vec<AnyView> = vec![AnyView::new(
        text(language)
            .size(11.0)
            .foreground(Color::srgb_hex("#64B5F6")),
    )];
    if !name.is_empty() {
        header_views.push(AnyView::new(
            text(name).size(11.0).foreground(Color::srgb_hex("#808080")),
        ));
    }

    let exec_ops: Vec<_> = ba
        .ctx
        .operations
        .iter()
        .filter(|ow| ow.descriptor.name == "execute_source_block")
        .collect();
    if let Some(exec_op) = exec_ops.first() {
        let row_id = holon_frontend::operations::get_row_id(ba.ctx);
        let entity_name = holon_frontend::operations::get_entity_name(ba.ctx)
            .unwrap_or_else(|| exec_op.descriptor.entity_name.to_string());
        let op_name = exec_op.descriptor.name.clone();
        let session = ba.ctx.session.clone();
        let handle = ba.ctx.runtime_handle.clone();
        header_views.push(AnyView::new(
            text("[run]")
                .size(11.0)
                .foreground(Color::srgb_hex("#4CAF50"))
                .on_tap(move || {
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
        ));
    }

    let mut all_views: Vec<AnyView> = vec![AnyView::new(hstack(header_views).spacing(8.0))];

    all_views.push(AnyView::new(
        text(source)
            .size(13.0)
            .foreground(Color::srgb_hex("#E0E0E0"))
            .padding(),
    ));

    AnyView::new(
        vstack(all_views)
            .spacing(4.0)
            .background(Color::srgb_hex("#1A1A2E")),
    )
}
