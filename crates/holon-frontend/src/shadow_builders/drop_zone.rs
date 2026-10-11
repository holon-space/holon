// A drop target. `op` names the row operation a drag released on this zone
// dispatches. The zone binds that operation from the row's offered operations,
// so a row that is not offered it (the write tier refuses the row) binds none
// and offers no drop. `op` is stamped as a prop because `render_entity` may
// union further row operations onto this node.
use super::prelude::*;

holon_macros::widget_builder! {
    fn drop_zone(#[default = "move_block"] op: String) {
        // `row_id()` must name the target row: the GPUI drop_zone builder and
        // the headless drop walker both match by it.
        ViewModel {
            data: futures_signals::signal::Mutable::new(ba.ctx.row_arc()).read_only(),
            operations: super::prelude::offered_operation(ba.ctx, &op),
            ..ViewModel::from_widget(
                "drop_zone",
                std::collections::HashMap::from([("op".to_string(), Value::String(op))]),
            )
        }
    }
}
