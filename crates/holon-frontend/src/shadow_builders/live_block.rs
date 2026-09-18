use super::prelude::*;

holon_macros::widget_builder! {
    raw fn live_block(ba: BA<'_>) -> ViewModel {
        match crate::render_interpreter::live_block_target(&ba) {
            Ok(block_id) => ViewModel::live_block(block_id),
            Err(msg) => ViewModel::error("live_block", msg),
        }
    }
}
