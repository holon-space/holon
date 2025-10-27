use holon_api::EntityUri;

use super::prelude::*;

holon_macros::widget_builder! {
    raw fn live_block(ba: BA<'_>) -> ViewModel {
        let block_id = ba
            .args
            .get_positional_string(0)
            .or_else(|| ba.ctx.row().get("id").and_then(|v| v.as_string()).map(|s| s.to_string()))
            .map(|s| EntityUri::parse(&s).expect("live_block: invalid entity URI"))
            .expect("live_block: no positional arg and no 'id' column in current row");

        ViewModel::live_block(block_id)
    }
}
