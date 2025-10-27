use super::prelude::*;
use crate::reactive_view_model::CollectionData;

holon_macros::widget_builder! {
    fn table(children: Collection) {
        let __parent_space = ba.ctx.available_space;
        match children {
            CollectionData::Streaming { item_template, data_source, sort_key } => {
                let virtual_child = virtual_child_slot_from_arg(&ba);
                ViewModel::streaming_collection("table", item_template, data_source, 4.0, sort_key, __parent_space, None, virtual_child, None)
            }
            CollectionData::Static { mut items } => {
                if let Some(tmpl) = ba.args.get_template("item_template").or(ba.args.get_template("item")) {
                    if let Some(vc) = interpret_virtual_child(&ba, tmpl) {
                        items.push(vc);
                    }
                }
                ViewModel::static_collection("table", items, 4.0)
            }
        }
    }
}
