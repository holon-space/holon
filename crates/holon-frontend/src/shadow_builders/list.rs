use super::prelude::*;
use crate::reactive_view_model::CollectionData;

holon_macros::widget_builder! {
    fn list(children: Collection) {
        let __parent_space = ba.ctx.available_space;
        // `horizontal: true` lays the items along a row — an integration row's
        // op buttons sit on one baseline so each reads as belonging to its row;
        // `wrap: "wrap"` lets that row continue on a second line, which a
        // container whose item count comes from the data needs.
        let layout = match CollectionVariant::parse("list", "list", &ba.args.named) {
            Ok(layout) => layout,
            Err(msg) => return ViewModel::error("list", msg),
        };
        let (gap, flow) = (layout.gap, layout.flow);
        match children {
            CollectionData::Streaming { item_template, data_source, sort_key, rules } => {
                let virtual_child = match virtual_child_slot_from_arg(&ba) {
                    Ok(slot) => slot,
                    Err(msg) => return ViewModel::error("list", msg),
                };
                ViewModel::streaming_collection("list", item_template, data_source, gap, flow, sort_key, __parent_space, None, virtual_child, rules, None, Default::default())
            }
            CollectionData::Static { mut items } => {
                if let Some(tmpl) = ba.args.get_template("item_template").or(ba.args.get_template("item")) {
                    let vc = match interpret_virtual_child(&ba, tmpl) {
                        Ok(vc) => vc,
                        Err(msg) => return ViewModel::error("list", msg),
                    };
                    if let Some(vc) = vc {
                        items.push(vc);
                    }
                }
                let items = weave_advice_into_items(&ba, items);
                ViewModel::static_collection("list", items, gap, flow, Default::default())
            }
        }
    }
}
