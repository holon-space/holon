use super::prelude::*;

holon_macros::widget_builder! {
    fn source_block(#[default = "text"] language: String, content: String, name: String, #[default = false] editable: bool);
}
