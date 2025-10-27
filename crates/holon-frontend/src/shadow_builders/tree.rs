use super::prelude::*;
use crate::render_interpreter::shared_tree_build;

/// Build the trailing-slot ViewModel for a tree's creation slot.
///
/// Activated by `creation_slot: true` in the args. Looks up the parent's
/// entity profile defaults via `BuilderServices::virtual_child_config`,
/// constructs a synthetic data row with `id: "virtual:<parent_id>"` plus the
/// defaults, and interprets the tree's `item_template` against that row.
///
/// The synthetic `virtual:` id is a private routing token: it lives only on
/// this slot's local `DataRow` and never enters the data source's SignalVec.
/// The existing `editable_text` → `EditorView` → `EditorViewModel.on_blur`
/// → `ViewEventHandler::handle_text_sync` → `parse_virtual_id` pipeline picks
/// it up at submit time and dispatches `<entity>.create` with the typed
/// content as the `content` field plus `parent_id`.
///
/// Returns `None` when `creation_slot` is absent, when no `virtual_parent`
/// is resolved into args (live_query already substitutes; live_block still
/// pending — see `block_domain.rs::collection_render_from_profile`), when
/// the surrounding entity has no `virtual_child` profile, or when the tree
/// has no `item_template`.
fn build_trailing_slot(ba: &BA<'_>) -> Option<crate::reactive_view::TrailingSlot> {
    let cs = ba.args.get_bool("creation_slot");
    let vp_str = ba.args.get_string("virtual_parent").map(|s| s.to_string());
    let has_template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
        .is_some();
    tracing::debug!(
        "[VIRTUAL_CHILD] build_trailing_slot: creation_slot={cs:?} virtual_parent={vp_str:?} \
         has_template={has_template}"
    );
    if !cs.unwrap_or(false) {
        tracing::debug!("[VIRTUAL_CHILD] -> None (creation_slot not true)");
        return None;
    }
    let Some(slot) = virtual_child_slot_from_arg(ba) else {
        tracing::debug!("[VIRTUAL_CHILD] -> None (virtual_child_slot_from_arg failed)");
        return None;
    };
    let Some(template) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    else {
        tracing::debug!("[VIRTUAL_CHILD] -> None (no item_template)");
        return None;
    };
    let row = virtual_child_row(&slot);
    let row_ctx = ba.ctx.with_row(row);
    let vm = (ba.interpret)(template, &row_ctx);
    tracing::debug!(
        "[VIRTUAL_CHILD] -> Some slot for parent_id={}",
        slot.parent_id
    );
    Some(crate::reactive_view::TrailingSlot {
        view_model: Arc::new(vm),
    })
}

holon_macros::widget_builder! {
    raw fn tree(ba: BA<'_>) -> ViewModel {
        tracing::debug!("[VIRTUAL_CHILD] tree::build dispatched! creation_slot={:?} virtual_parent={:?}",
            ba.args.get_bool("creation_slot"),
            ba.args.get_string("virtual_parent"));
        let __template = ba.args.get_template("item_template")
            .or(ba.args.get_template("item"))
            .cloned();
        let __sort_key: Option<String> = holon_api::render_eval::sort_key_column(ba.args)
            .map(|s| s.to_string());

        let __parent_space = ba.ctx.available_space;
        let __trailing_slot = build_trailing_slot(&ba);
        match (__template, ba.ctx.data_source.clone()) {
            (Some(tmpl), Some(ds)) => {
                // Inject the creation placeholder as a REACTIVE virtual row
                // (VirtualChildRowProvider) rather than a static `TrailingSlot`
                // snapshot. A snapshot is interpreted once (unfocused →
                // read-only `rendered_text`) and never re-resolves on focus, so
                // clicking the trailing placeholder set focus but it never
                // became an editable `EditorView` — no caret, typing did
                // nothing. As a real row it re-resolves `rendered_text` →
                // `editable_text` on focus exactly like the other rows. The
                // static `TrailingSlot` is still used for the no-data_source
                // (live-query/static) branch below, where reactive injection
                // isn't available.
                let _ = &__trailing_slot;
                let virtual_child = virtual_child_slot_from_arg(&ba);
                let __rules = crate::row_pipeline::parse_rules_arg(ba.args.named.get("rules"));
                ViewModel::streaming_collection("tree", tmpl, ds, 4.0, __sort_key, __parent_space, None, virtual_child, None, __rules)
            }
            _ => {
                let mut flat: Vec<(ViewModel, usize, std::collections::HashMap<String, Value>)> =
                    shared_tree_build(&ba);
                // Push the trailing slot BEFORE the empty check so the slot
                // renders even for empty collections — the user needs to be
                // able to create the first child via the slot. Prefer the
                // `creation_slot: true`-driven slot; fall back to legacy
                // `virtual_parent` only when the new flag is absent.
                if let Some(slot) = __trailing_slot {
                    if let Ok(inner) = std::sync::Arc::try_unwrap(slot.view_model) {
                        flat.push((inner, 0, std::collections::HashMap::new()));
                    }
                } else if let Some(tmpl) = ba.args.get_template("item_template").or(ba.args.get_template("item")) {
                    if let Some(vc) = interpret_virtual_child(&ba, tmpl) {
                        flat.push((vc, 0, std::collections::HashMap::new()));
                    }
                }
                if flat.is_empty() {
                    return ViewModel::leaf("text", Value::String("[tree: no item_template]".into()));
                }
                ViewModel::static_collection("tree", flat_tree_items(flat), 4.0)
            }
        }
    }
}

/// Convert a flat depth-first `(node, depth, overrides)` list into flat
/// `TreeItem` wrappers. Each item carries its depth for indentation and a
/// `has_children` flag for the collapse chevron; `has_children` is true
/// when the next item has a greater depth. The per-row override map (from
/// tree builder rules: evaluation) is merged into the resulting tree_item's
/// props alongside `depth` / `has_children`, so chrome-affecting keys like
/// `show_bullet` and `show_chevron` reach the frontend's tree_item builder.
pub fn flat_tree_items(
    flat: Vec<(ViewModel, usize, std::collections::HashMap<String, Value>)>,
) -> Vec<ViewModel> {
    let len = flat.len();
    let depths: Vec<usize> = flat.iter().map(|(_, d, _)| *d).collect();
    flat.into_iter()
        .enumerate()
        .map(|(i, (node, depth, overrides))| {
            let has_children = i + 1 < len && depths[i + 1] > depth;
            let entity = node.entity();
            let vm = ViewModel::tree_item(node, depth, has_children);
            if !overrides.is_empty() {
                let mut p = vm.props.lock_mut();
                for (k, v) in overrides {
                    p.insert(k, v);
                }
            }
            vm.with_entity(entity)
        })
        .collect()
}
