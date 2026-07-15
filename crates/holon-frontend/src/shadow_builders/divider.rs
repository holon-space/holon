use super::prelude::*;
use crate::render_context::LayoutHint;

holon_macros::widget_builder! {
    raw fn divider(_ba: BA<'_>) -> ViewModel {
        ViewModel {
            layout_hint: LayoutHint::Flex { weight: 0.0 },
            ..ViewModel::from_widget("divider", std::collections::HashMap::new())
        }
    }
}
