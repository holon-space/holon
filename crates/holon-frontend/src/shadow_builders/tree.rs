use super::prelude::*;
use crate::render_interpreter::TreeInputs;
use crate::render_interpreter::shared_tree_build;

holon_macros::widget_builder! {
    raw fn tree(ba: BA<'_>) -> ViewModel {
        tracing::debug!("[VIRTUAL_CHILD] tree::build dispatched! creation_slot={:?} virtual_parent={:?}",
            ba.args.get_bool("creation_slot"),
            ba.args.get_string("virtual_parent"));
        // These three become typed `Expr` params when `tree` migrates off
        // `raw fn`; the helpers they feed no longer read the arg bag.
        let __template = ba.args.get_template("item_template")
            .or(ba.args.get_template("item"));
        let __parent_id = ba.args.get_template("parent_id");
        let __sortkey = ba.args.get_template("sortkey")
            .or(ba.args.get_template("sort_key"));

        let __sort_key: Option<String> = holon_api::render_eval::sort_key_column(ba.args)
            .map(|s| s.to_string());

        let __parent_space = ba.ctx.available_space;
        match (__template, ba.ctx.data_source.clone()) {
            (Some(tmpl), Some(ds)) => {
                // Streaming path: the creation placeholder is injected as a
                // REACTIVE virtual row via `AppendedRowsProvider::creation_slot`
                // (wired in the collection driver from `virtual_child`), NOT as a
                // ViewModel snapshot. A snapshot is interpreted once (unfocused →
                // read-only `rendered_text`) and never re-resolves on focus; as a
                // real row it re-resolves `rendered_text` → `editable_text` on
                // focus exactly like the other rows.
                let virtual_child = virtual_child_slot_from_arg(&ba);
                let __rules = crate::row_pipeline::parse_rules_arg(ba.args.named.get("rules"));
                ViewModel::streaming_collection("tree", tmpl.clone(), ds, 4.0, false, __sort_key, __parent_space, None, virtual_child, __rules, Default::default())
            }
            (Some(tmpl), None) => {
                let mut flat: Vec<(ViewModel, usize, std::collections::HashMap<String, Value>)> =
                    shared_tree_build(&ba, &TreeInputs::new(tmpl, __parent_id, __sortkey));
                // Push the creation slot BEFORE the empty check so it renders even
                // for empty collections — the user needs to create the first child
                // via the slot. Static/snapshot path only (live-query / MCP / PBT);
                // the streaming path above injects it as a reactive row instead.
                if let Some(vc) = interpret_virtual_child(&ba, tmpl) {
                    flat.push((vc, 0, std::collections::HashMap::new()));
                }
                if flat.is_empty() {
                    return ViewModel::leaf("text", Value::String("[tree: no item_template]".into()));
                }
                let items = weave_advice_into_items(&ba, flat_tree_items(flat));
                ViewModel::static_collection("tree", items, 4.0, false, Default::default())
            }
            (None, _) => {
                ViewModel::leaf("text", Value::String("[tree: no item_template]".into()))
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
