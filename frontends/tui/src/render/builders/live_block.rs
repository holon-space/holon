use super::prelude::*;
use holon_frontend::render_interpreter::shared_live_block_build;

pub fn build(ba: BA<'_>) -> TuiWidget {
    match shared_live_block_build(&ba) {
        Ok(widget) => widget,
        Err(msg) => TuiWidget::Text {
            content: msg,
            bold: false,
        },
    }
}
