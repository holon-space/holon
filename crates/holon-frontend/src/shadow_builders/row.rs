use super::prelude::*;

holon_macros::widget_builder! {
    fn row(#[default = 8.0] gap: f32, children: Collection);
}
