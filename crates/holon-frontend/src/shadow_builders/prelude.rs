pub(crate) use std::sync::Arc;

pub(crate) use holon_api::Value;

pub(crate) use crate::reactive_view_model::ReactiveViewModel as ViewModel;
pub(crate) use crate::render_interpreter::BuilderArgs;

pub(crate) type BA<'a> = BuilderArgs<'a, crate::reactive_view_model::ReactiveViewModel>;

/// Compute the `VirtualChildSlot` from a `virtual_parent` arg string or
/// context default — `None` for a collection that did not ask for a slot.
///
/// The trailing "type here to create" slot is OPT-IN: a collection declares it
/// with `creation_slot: true` (the `tree_view` variant of
/// `assets/default/types/collection_profile.yaml`). Read-only navigation trees
/// — the left sidebar's page list, the right sidebar's outline mirror — omit
/// the flag and get no slot AT ALL. Gating here rather than inside
/// `resolve_creation_parent` is what makes the omission mean something: the
/// `virtual_parent` fallbacks below derive the container FROM the rendered
/// rows, so the flat-shape test in `resolve_creation_parent` is satisfied by
/// construction and every later gate is unreachable (that is how a
/// `block:__virtual:<page>` phantom row reached Martin's sidebar, desyncing the
/// tree provider from its `row_map` on each disclosure toggle).
///
/// When `virtual_parent` is an explicit string arg (resolved from the
/// `Bool(true)` sentinel by `resolve_virtual_parent`), use it. Otherwise,
/// fall back to the context row's `id` column — which is the surrounding
/// entity when a tree collection is being built inside a `render_entity` or
/// `live_block` render. This default lets the PBT's explicit render-source
/// expressions (which have no sentinel resolution) still produce a slot.
pub(crate) fn virtual_child_slot_from_arg(
    ba: &BA<'_>,
) -> Option<crate::reactive_view::VirtualChildSlot> {
    // `creation_slot: true` ALSO gates the top-level "create a new root entity"
    // slot for a flat `no_parent` forest (BugFunnel #61 / #67), which
    // `resolve_creation_parent` reads as `allow_root_creation`.
    let creation_slot = ba.args.get_bool("creation_slot").unwrap_or(false);
    if !creation_slot {
        return None;
    }
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
    // ALLOW(entity_uri_from_raw): render-spec arg or data row 'parent_id'
    let uri = holon_api::EntityUri::from_raw(&vp);
    let entity_name = uri.scheme().to_string();
    let config = ba.services.virtual_child_config(&entity_name)?;
    Some(crate::reactive_view::VirtualChildSlot {
        defaults: config.defaults,
        parent_id: uri,
        allow_root_creation: creation_slot,
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
    parent: &holon_api::EntityUri,
    defaults: &std::collections::HashMap<String, holon_api::Value>,
) -> Arc<holon_api::widget_spec::DataRow> {
    let virtual_key = crate::row_origin::RowOrigin::creation_placeholder_id(parent);
    // Defaults FIRST — see `creation_slot_keyed_row`: they carry the declared
    // schema, the structural columns below must win.
    let mut row: std::collections::HashMap<String, Value> = defaults.clone();
    row.insert("id".to_string(), Value::String(virtual_key));
    row.insert(
        "parent_id".to_string(),
        Value::String(parent.as_str().to_string()),
    );
    row.insert("sort_key".to_string(), Value::Float(f64::MAX));
    Arc::new(row)
}

/// Interpret a virtual child row through a template and return the ViewModel.
///
/// Used by collection builders in the static/snapshot path (signal
/// re-interpretation) where items are eagerly interpreted from data rows.
/// The `template` argument is the collection's `item_template` and is
/// deliberately IGNORED: an affordance is a rendered row, not a block, so it
/// renders through the read-only
/// [`creation_affordance_template`](crate::reactive_view::creation_affordance_template)
/// — never the editable item template — exactly as the streaming path does.
/// The parameter stays for call-site symmetry with the item interpretation
/// beside it.
pub(crate) fn interpret_virtual_child(
    ba: &BA<'_>,
    template: &holon_api::render_types::RenderExpr,
) -> Option<ViewModel> {
    let _ = template;
    let template = &crate::reactive_view::creation_affordance_template();
    let slot = virtual_child_slot_from_arg(ba)?;
    // Bug 2A: parent the creation slot at the query's focus root (resolved from
    // the rendered rows), not the static container `slot.parent_id`. `None`
    // (empty / not-yet-resolvable) → no slot rather than a silent mis-parent.
    let parent = crate::row_origin::resolve_creation_parent(
        &ba.ctx.data_rows,
        &slot.parent_id,
        slot.allow_root_creation,
    )?;
    let row = virtual_child_row(&parent, &slot.defaults);
    let row_ctx = ba.ctx.with_row(row);
    Some((ba.interpret)(template, &row_ctx))
}

/// Weave the session-level advice sidecar (ADR 0022) into a static collection's
/// items on the PURE/snapshot path — the read side of the sidecar the reactive
/// weaver / composed settle populate (`crate::advice_weaver`).
///
/// For each item whose id is an anchor with woven advice, the advice rows are
/// interpreted through the READ-ONLY advice template (never the editable
/// `item_template`) and appended as DIRECT children of the anchor item, each
/// stamped with its `Occurrence::Placed(for_placement(lesson, anchor))`
/// coordinate. Appending under the anchor item (not as flat siblings) is what
/// makes the woven rows show up under `node.children` where the keystone
/// snapshot invariant (`inv-advice-rows-woven`) reads them. A byte-for-byte
/// no-op when the sidecar has no entry for the anchor (the common case).
pub(crate) fn weave_advice_into_items(ba: &BA<'_>, items: Vec<ViewModel>) -> Vec<ViewModel> {
    items
        .into_iter()
        .map(|item| weave_advice_into_item(ba, item))
        .collect()
}

fn weave_advice_into_item(ba: &BA<'_>, mut item: ViewModel) -> ViewModel {
    let Some(anchor_id) = item.row_id() else {
        return item;
    };
    let Ok(anchor) = holon_api::EntityUri::parse(&anchor_id) else {
        return item;
    };
    let advice_rows = ba.services.advice_children(&anchor);
    if advice_rows.is_empty() {
        return item;
    }
    let template = crate::reactive_view::advice_readonly_template();
    for row in advice_rows {
        let occurrence = crate::advice_weaver::advice_row_occurrence(&row);
        let row_ctx = ba.ctx.with_row(row);
        let mut vm = (ba.interpret)(&template, &row_ctx);
        vm.occurrence = occurrence;
        item.children.push(Arc::new(vm));
    }
    item
}

/// Re-derive a leaf widget's `content` prop from one column of a CDC row.
///
/// `None` means the row does not carry `field` at all — a pre-first-batch
/// empty row, or a `col()` name outside the projection. The caller keeps its
/// build-time snapshot instead of blanking the widget.
///
/// A present value is stringified deliberately (`to_display_string`), the same
/// coercion the build-time snapshot applies via
/// `ResolvedArgs::get_positional_string`: an INTEGER column renders its digits
/// and only SQL NULL renders empty.
pub(crate) fn content_from_row(
    row: &holon_api::widget_spec::DataRow,
    field: &str,
) -> Option<String> {
    row.get(field).map(|v| v.to_display_string())
}
