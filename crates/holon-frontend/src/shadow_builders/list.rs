use super::prelude::*;
use crate::reactive_view_model::CollectionData;

holon_macros::widget_builder! {
    fn list(#[default = 4.0] gap: f32, children: Collection) {
        let __parent_space = ba.ctx.available_space;
        // Lay the items along a row instead of the default stacked column — an
        // integration row's op buttons sit on one baseline so each reads as
        // belonging to its row.
        let horizontal = ba.args.get_bool("horizontal").unwrap_or(false);
        match children {
            CollectionData::Streaming { item_template, data_source, sort_key, rules } => {
                let virtual_child = virtual_child_slot_from_arg(&ba);
                ViewModel::streaming_collection("list", item_template, data_source, gap, horizontal, sort_key, __parent_space, None, virtual_child, rules, None, Default::default())
            }
            CollectionData::Static { mut items } => {
                if let Some(tmpl) = ba.args.get_template("item_template").or(ba.args.get_template("item")) {
                    if let Some(vc) = interpret_virtual_child(&ba, tmpl) {
                        items.push(vc);
                    }
                }
                let items = weave_advice_into_items(&ba, items);
                ViewModel::static_collection("list", items, gap, horizontal, Default::default())
            }
        }
    }
}
