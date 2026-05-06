use super::prelude::*;
use crate::shadow_builders::tree::flat_tree_items;
use crate::render_interpreter::shared_tree_build;

holon_macros::widget_builder! {
    raw fn outline(ba: BA<'_>) -> ViewModel {
        let __template = ba.args.get_template("item_template")
            .or(ba.args.get_template("item"))
            .cloned();
        let __sort_key: Option<String> = holon_api::render_eval::sort_key_column(ba.args)
            .map(|s| s.to_string());
        let __rules = crate::row_pipeline::parse_rules_arg(ba.args.named.get("rules"));

        let __parent_space = ba.ctx.available_space;
        match (__template, ba.ctx.data_source.clone()) {
            (Some(tmpl), Some(ds)) => {
                let virtual_child = virtual_child_slot_from_arg(&ba);
                ViewModel::streaming_collection("outline", tmpl, ds, 4.0, __sort_key, __parent_space, None, virtual_child, None, __rules)
            }
            _ => {
                let mut flat: Vec<(ViewModel, usize, std::collections::HashMap<String, Value>)> =
                    shared_tree_build(&ba);
                if flat.is_empty() {
                    return ViewModel::error("outline", "no item_template");
                }
                if let Some(tmpl) = ba.args.get_template("item_template").or(ba.args.get_template("item")) {
                    if let Some(vc) = interpret_virtual_child(&ba, tmpl) {
                        flat.push((vc, 0, std::collections::HashMap::new()));
                    }
                }
                ViewModel::static_collection("outline", flat_tree_items(flat), 4.0)
            }
        }
    }
}
