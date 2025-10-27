use super::prelude::*;
use crate::render_interpreter::shared_tree_build;

holon_macros::widget_builder! {
    raw fn tree(ba: BA<'_>) -> ViewModel {
        let __template = ba.args.get_template("item_template")
            .or(ba.args.get_template("item"))
            .cloned();
        let __sort_key: Option<String> = holon_api::render_eval::sort_key_column(ba.args)
            .map(|s| s.to_string());

        let __parent_space = ba.ctx.available_space;
        match (__template, ba.ctx.data_source.clone()) {
            (Some(tmpl), Some(ds)) => {
                let virtual_child = virtual_child_slot_from_arg(&ba);
                ViewModel::streaming_collection("tree", tmpl, ds, 4.0, __sort_key, __parent_space, None, virtual_child)
            }
            _ => {
                let mut flat: Vec<(ViewModel, usize)> = shared_tree_build(&ba);
                if flat.is_empty() {
                    return ViewModel::leaf("text", Value::String("[tree: no item_template]".into()));
                }
                if let Some(tmpl) = ba.args.get_template("item_template").or(ba.args.get_template("item")) {
                    if let Some(vc) = interpret_virtual_child(&ba, tmpl) {
                        flat.push((vc, 0));
                    }
                }
                ViewModel::static_collection("tree", flat_tree_items(flat), 4.0)
            }
        }
    }
}

/// Convert a flat depth-first `(node, depth)` list into flat `TreeItem` wrappers.
/// Each item carries its depth for indentation and a `has_children` flag for
/// the collapse chevron. `has_children` is true when the next item has a greater depth.
pub fn flat_tree_items(flat: Vec<(ViewModel, usize)>) -> Vec<ViewModel> {
    let len = flat.len();
    let depths: Vec<usize> = flat.iter().map(|(_, d)| *d).collect();
    flat.into_iter()
        .enumerate()
        .map(|(i, (node, depth))| {
            let has_children = i + 1 < len && depths[i + 1] > depth;
            let entity = node.entity();
            ViewModel::tree_item(node, depth, has_children).with_entity(entity)
        })
        .collect()
}
