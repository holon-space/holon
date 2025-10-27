use super::prelude::*;

holon_macros::widget_builder! {
    fn list(#[default = 4.0] gap: f32, children: Collection);
}
