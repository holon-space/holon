use super::prelude::*;
use holon_frontend::render_interpreter::shared_block_ref_build;

pub fn build(ba: BA<'_>) -> TuiWidget {
    match shared_block_ref_build(&ba) {
        Ok(widget) => widget,
        Err(msg) => TuiWidget::Text {
            content: msg,
            bold: false,
        },
    }
}
