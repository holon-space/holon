use super::prelude::*;

holon_macros::widget_builder! {
    fn source_editor(#[default = "text"] language: String, content: String);
}
