pub(crate) use std::sync::Arc;

pub(crate) use crate::reactive_view_model::ReactiveViewModel as ViewModel;
pub(crate) use crate::render_interpreter::BuilderArgs;
pub(crate) use holon_api::Value;

pub(crate) type BA<'a> = BuilderArgs<'a, crate::reactive_view_model::ReactiveViewModel>;

/// Compute the `VirtualChildSlot` from a `virtual_parent` arg string.
///
/// Returns `Some(slot)` when the parent entity's profile declares `virtual_child`.
/// Used by collection builders (tree, list, table, outline) in both streaming
/// and static paths.
pub(crate) fn virtual_child_slot_from_arg(
    ba: &BA<'_>,
) -> Option<crate::reactive_view::VirtualChildSlot> {
    let vp = ba.args.get_string("virtual_parent")?;
    let uri = holon_api::EntityUri::from_raw(vp);
    let entity_name = uri.scheme().to_string();
    let config = ba.services.virtual_child_config(&entity_name)?;
    Some(crate::reactive_view::VirtualChildSlot {
        defaults: config.defaults,
        parent_id: vp.to_string(),
    })
}

/// Build a virtual child DataRow from a `VirtualChildSlot`.
///
/// The row uses synthetic ID `block:virtual:{parent_id}`, `sort_key: MAX`
/// (sorts last), and all defaults from the entity profile.
pub(crate) fn virtual_child_row(
    slot: &crate::reactive_view::VirtualChildSlot,
) -> Arc<holon_api::widget_spec::DataRow> {
    let virtual_key = format!("virtual:{}", slot.parent_id);
    let mut row = std::collections::HashMap::new();
    row.insert("id".to_string(), Value::String(virtual_key));
    row.insert(
        "parent_id".to_string(),
        Value::String(slot.parent_id.clone()),
    );
    row.insert("sort_key".to_string(), Value::Float(f64::MAX));
    for (k, v) in &slot.defaults {
        row.insert(k.clone(), v.clone());
    }
    Arc::new(row)
}

/// Interpret a virtual child row through a template and return the ViewModel.
///
/// Used by collection builders in the static/snapshot path (signal
/// re-interpretation) where items are eagerly interpreted from data rows.
pub(crate) fn interpret_virtual_child(
    ba: &BA<'_>,
    template: &holon_api::render_types::RenderExpr,
) -> Option<ViewModel> {
    let slot = virtual_child_slot_from_arg(ba)?;
    let row = virtual_child_row(&slot);
    let row_ctx = ba.ctx.with_row(row);
    Some((ba.interpret)(template, &row_ctx))
}
