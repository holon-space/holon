use holon_frontend::ReactiveViewModel;

use super::prelude::*;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let message = node.prop_str("message").unwrap_or_default();
    // A disclosed degradation reads calmer than a failure: the block is fine,
    // the integration it reads from is not running, and the remedy is the
    // integration's. Painting it danger-red would make a known, named state
    // look like a crash.
    let disclosed = node.prop_str("degraded_disclosure").is_some();
    let painted = div()
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
        .child(message.clone())
        .into_any_element();

    // The message rides a tracker of its own so a windowed assertion can read
    // what the row says, the way `error_banner` lets it for the builders that
    // own a failure of their own. Its own widget type, so the row still counts
    // as ONE `error` element.
    crate::geometry::tracked(
        format!("error-message-{message}"),
        painted,
        &ctx.bounds_registry,
        "error_message",
        None,
        true,
        Some(message.into()),
    )
    .into_any_element()
}
