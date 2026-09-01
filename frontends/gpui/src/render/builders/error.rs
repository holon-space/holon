use holon_frontend::ReactiveViewModel;

use super::prelude::*;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let message = node.prop_str("message").unwrap_or_default();
    // A disclosed degradation reads calmer than a failure: the block is fine,
    // the integration it reads from is not running, and the remedy is the
    // integration's. Painting it danger-red would make a known, named state
    // look like a crash.
    let disclosed = node.prop_str("degraded_disclosure").is_some();
    div()
        .p_2()
        .rounded(px(4.0))
        .bg(tc(ctx, |t| t.secondary))
        .text_color(tc(ctx, |t| {
            if disclosed {
                t.muted_foreground
            } else {
                t.danger
            }
        }))
        .text_sm()
        .child(message)
}
