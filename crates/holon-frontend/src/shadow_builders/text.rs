use super::prelude::*;

holon_macros::widget_builder! {
    fn text(content: String, #[default = false] bold: bool, #[default = 14.0] size: f32, color: Option<String>);
}
