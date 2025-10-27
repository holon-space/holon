use super::prelude::*;

// One lane in a `board(...)` widget. `title` is the lane header; positional
// children are the lane's cards in initial order. The GPUI renderer feeds them
// into a `SortableState<T>` the first time the lane is mounted, after which
// drag-and-drop owns the order.
holon_macros::widget_builder! {
    fn board_lane(#[default = "Lane"] title: String, children: Collection) {
        let mut __props = std::collections::HashMap::new();
        __props.insert("title".to_string(), Value::String(title));
        ViewModel {
            children: children.into_static_items().into_iter().map(Arc::new).collect(),
            ..ViewModel::from_widget("board_lane", __props)
        }
    }
}
