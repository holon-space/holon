use super::prelude::*;
use crate::reactive_view_model::CollectionData;

holon_macros::widget_builder! {
    fn list(#[default = 4.0] gap: f32, children: Collection) {
        let __parent_space = ba.ctx.available_space;
        // Lay the items along a row instead of the default stacked column — an
        // integration row's op buttons sit on one baseline so each reads as
        // belonging to its row. `wrap: "wrap"` lets that row continue on a
        // second line when it runs short of width, which a container whose item
        // count comes from the data (one button per operation an entity
        // advertises) needs and a fixed column width cannot supply.
        //
        // Both keywords are parsed HERE, at the build boundary: an unknown one
        // is a config error, and a wrap mode that fell back to the default
        // would put a button outside its column with nothing saying so.
        // The RAW values, not `get_bool`/`get_string`: those accessors answer
        // `None` both for "absent" and for "present but of the wrong type",
        // which is how a `wrap: true` used to lay the collection out unwrapped
        // in silence. `ItemFlow::parse` is the one place either reader judges
        // them.
        let flow = ItemFlow::parse(ba.args.named.get("horizontal"), ba.args.named.get("wrap"));
        // The macro's `gap` param comes from `get_f64`, which has the same
        // wrong-type-reads-as-absent shape, so the declared default would
        // silently stand in for a mistyped one. Re-read it strictly through the
        // parse both readers share; an absent `gap:` keeps the macro's default.
        let gap = crate::reactive_view_model::parse_gap(ba.args.named.get("gap"), gap);
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
