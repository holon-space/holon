use super::prelude::*;
use crate::shadow_builders::tree::nest_by_depth;
use crate::render_interpreter::shared_tree_build;

holon_macros::widget_builder! {
    raw fn outline(ba: BA<'_>) -> ViewModel {
        let flat: Vec<(ViewModel, usize)> = shared_tree_build(&ba);

        if flat.is_empty() {
            return ViewModel::error("outline", "no item_template");
        }

        let items = nest_by_depth(flat);
        ViewModel::collection("outline", items)
    }
}
