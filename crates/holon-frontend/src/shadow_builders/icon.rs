use super::prelude::*;

holon_macros::widget_builder! {
    fn icon(#[default = "circle"] name: String, #[default = 16.0] size: f32);
}
