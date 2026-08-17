//! The track-and-knob switch, painted in ONE place.
//!
//! Two surfaces show a switch in the same Settings modal — the preference
//! toggle and a `state_toggle` with `appearance: "switch"` — and the two read
//! as one control only if they are one control. This module is that control;
//! neither caller carries its own geometry.

use super::prelude::*;

/// The control's own box. Exported so a caller can size its wrapper to the
/// track rather than letting a block-level parent stretch the switch across
/// the row — and so a windowed test names the same numbers the paint does.
pub const TRACK_WIDTH: f32 = 36.0;
pub const TRACK_HEIGHT: f32 = 20.0;

/// The switch, in `on` or `off`. Caller owns the click and the element id: this
/// is the painted control, not the interaction.
pub fn switch_track(ctx: &GpuiRenderContext, on: bool) -> Div {
    let (track_bg, knob_offset) = if on {
        (tc(ctx, |t| t.success), px(18.0))
    } else {
        (gpui::hsla(0.0, 0.0, 1.0, 0.2), px(2.0))
    };

    div()
        .w(px(TRACK_WIDTH))
        .h(px(TRACK_HEIGHT))
        .rounded(px(10.0))
        .bg(track_bg)
        .relative()
        .child(
            div()
                .absolute()
                .top(px(2.0))
                .left(knob_offset)
                .w(px(16.0))
                .h(px(16.0))
                .rounded(px(8.0))
                .bg(gpui::rgba(0xffffffee)),
        )
}
