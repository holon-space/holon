pub(crate) use std::sync::Arc;

pub(crate) use crate::reactive_view_model::ReactiveViewModel as ViewModel;
pub(crate) use crate::render_interpreter::BuilderArgs;
pub(crate) use holon_api::Value;

pub(crate) type BA<'a> = BuilderArgs<'a, crate::reactive_view_model::ReactiveViewModel>;

/// Compute the `VirtualChildSlot` from a `virtual_parent` arg string or
/// context fallback.
///
/// When `virtual_parent` is an explicit string arg (resolved from the
/// `Bool(true)` sentinel by `resolve_virtual_parent`), use it. Otherwise,
/// fall back to the context row's `id` column — which is the surrounding
/// entity when a tree collection is being built inside a `render_entity` or
/// `live_block` render. This fallback lets the PBT's explicit render-source
/// expressions (which have no sentinel resolution) still produce a slot.
pub(crate) fn virtual_child_slot_from_arg(
    ba: &BA<'_>,
) -> Option<crate::reactive_view::VirtualChildSlot> {
    let vp = ba
        .args
        .get_string("virtual_parent")
        .map(|s| s.to_string())
        // Streaming path: context row IS the parent block.
        .or_else(|| {
            ba.ctx
                .row()
                .get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
        // Static/snapshot path: context rows are children; first
        // row's parent_id is the common parent.
        .or_else(|| {
            ba.ctx
                .data_rows
                .first()
                .and_then(|r| r.get("parent_id"))
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })?;
    let uri = holon_api::EntityUri::from_raw(&vp);
    let entity_name = uri.scheme().to_string();
    let config = ba.services.virtual_child_config(&entity_name)?;
    Some(crate::reactive_view::VirtualChildSlot {
        defaults: config.defaults,
        parent_id: vp.to_string(),
    })
}

/// Build a virtual child DataRow from a `VirtualChildSlot`.
///
/// The synthetic id is `<parent_scheme>:__virtual:<parent_local>` so
/// `EntityUri::scheme()` returns the parent's entity type (e.g. `"block"`)
/// — keeping the profile resolver happy. The `__virtual` marker lives in the
/// **local** part of the URI, not the scheme, so it doesn't get parsed as an
/// entity type. `parse_virtual_id` (`view_event_handler.rs`) recognises this
/// shape and dispatches `<entity>.create` on submit.
///
/// `sort_key: MAX` keeps the row sorted last; the `defaults` HashMap from the
/// entity profile fills in the rest of the columns.
pub(crate) fn virtual_child_row(
    slot: &crate::reactive_view::VirtualChildSlot,
) -> Arc<holon_api::widget_spec::DataRow> {
    let parent_uri = holon_api::EntityUri::from_raw(&slot.parent_id);
    let virtual_key = format!("{}:__virtual:{}", parent_uri.scheme(), parent_uri.id());
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
