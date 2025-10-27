use super::prelude::*;

holon_macros::widget_builder! {
    fn checkbox(#[default = false] checked: bool);
}
