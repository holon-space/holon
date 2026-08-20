use super::prelude::*;
use crate::render_interpreter::TreeInputs;
use crate::render_interpreter::shared_tree_build;
use crate::shadow_builders::tree::flat_tree_items;

holon_macros::widget_builder! {
    raw fn outline(ba: BA<'_>) -> ViewModel {
        // These three become typed `Expr` params when `outline` migrates off
        // `raw fn`; the helpers they feed no longer read the arg bag.
        let __template = ba.args.get_template("item_template")
            .or(ba.args.get_template("item"));
        let __parent_id = ba.args.get_template("parent_id");
        let __sortkey = ba.args.get_template("sortkey")
            .or(ba.args.get_template("sort_key"));

        let __sort_key: Option<String> = holon_api::render_eval::sort_key_column(ba.args)
            .map(|s| s.to_string());
        let __rules = crate::row_pipeline::parse_rules_arg(ba.args.named.get("rules"));

        let __parent_space = ba.ctx.available_space;
        match (__template, ba.ctx.data_source.clone()) {
            (Some(tmpl), Some(ds)) => {
                let virtual_child = virtual_child_slot_from_arg(&ba);
                ViewModel::streaming_collection("outline", tmpl.clone(), ds, 4.0, false, __sort_key, __parent_space, None, virtual_child, __rules, Default::default())
            }
            (Some(tmpl), None) => {
                let mut flat: Vec<(ViewModel, usize, std::collections::HashMap<String, Value>)> =
                    shared_tree_build(&ba, &TreeInputs::new(tmpl, __parent_id, __sortkey));
                if flat.is_empty() {
                    return ViewModel::error("outline", "no item_template");
                }
                if let Some(vc) = interpret_virtual_child(&ba, tmpl) {
                    flat.push((vc, 0, std::collections::HashMap::new()));
                }
                let items = weave_advice_into_items(&ba, flat_tree_items(flat));
                ViewModel::static_collection("outline", items, 4.0, false, Default::default())
            }
            (None, _) => ViewModel::error("outline", "no item_template"),
        }
    }
}
