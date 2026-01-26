use super::prelude::*;
use holon_frontend::ReactiveViewModel;

/// Two-slot anchored layout for the mobile action bar.
///
/// Layout: vertical flex. `children[0]` gets `flex_1` + `min_h_0` so it
/// consumes the remaining space (same idiom as `scrollable_list_wrapper`);
/// `children[1]` sits at its intrinsic height at the bottom of the content
/// area with the bottom safe-area inset applied as padding.
///
/// Why padding rather than absolute positioning: Android's
/// `adjustResize` shrinks the content rect when the IME opens, so the
/// dock naturally rises with the keyboard without any IME-specific code.
/// `safe_area_bottom_px()` only reserves the nav-bar / home-indicator
/// inset; on desktop it returns `0.0` so this widget still works outside
/// mobile (MCP / desktop testing).
pub fn render(
    node: &ReactiveViewModel,
    ctx: &GpuiRenderContext,
) -> Div {
    let children = &node.children;
    assert_eq!(
        children.len(),
        2,
        "bottom_dock requires exactly 2 children (main, dock); got {}",
        children.len()
    );
    let main = super::render(&children[0], ctx);
    let dock = super::render(&children[1], ctx);

    let bottom_inset = safe_area_bottom_logical_px();

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_hidden()
                .child(main),
        )
        .child(div().w_full().pb(px(bottom_inset)).child(dock))
}

/// Logical-px bottom safe-area inset (nav bar / home indicator). Folds in
/// IME height on Android when `adjustResize` is active. `0.0` on desktop.
fn safe_area_bottom_logical_px() -> f32 {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        crate::mobile::safe_area_bottom_px()
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        0.0
    }
}
