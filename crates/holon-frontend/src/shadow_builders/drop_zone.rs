// A drop target. `op` is the operation dispatched when a drag is
// released on this zone (defaults to `move_block`). The headless
// `UserDriver::drop_entity` and the GPUI `drop_zone` builder both
// read this so the wiring stays in one declarative place.
use super::prelude::*;

holon_macros::widget_builder! {
    fn drop_zone(#[default = "move_block"] op: String) {
        let mut __props = std::collections::HashMap::new();
        __props.insert("op".to_string(), Value::String(op));
        // Bind data to the current row so `row_id()` returns the target
        // block's id. The headless drop walker (drop_entity, inv16) and
        // the GPUI drop_zone builder match by row_id; both fall through
        // silently without this.
        ViewModel {
            data: futures_signals::signal::Mutable::new(ba.ctx.row_arc()).read_only(),
            ..ViewModel::from_widget("drop_zone", __props)
        }
    }
}
