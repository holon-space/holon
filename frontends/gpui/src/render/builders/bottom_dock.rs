use holon_frontend::ReactiveViewModel;

use super::prelude::*;

/// Height of the SOFT KEYBOARD in logical px, republished by `HolonApp::render`
/// every frame. `0.0` means the keyboard is down.
///
/// Deliberately NOT the bottom safe-area inset. That inset is the total
/// unusable bottom strip and is non-zero on every real phone with the keyboard
/// down — a home indicator, a nav bar, a gesture area — so gating the bar on it
/// would leave the bar permanently on screen. The page container still pads by
/// the total inset, which is correct for layout; only the "is the keyboard up"
/// question reads this.
///
/// Republished from `HolonApp` rather than read from the platform here so that
/// windowed tests can drive it through `RebindHandle::set_keyboard_height`.
#[derive(Default)]
pub(crate) struct KeyboardHeight(pub f32);

impl gpui::Global for KeyboardHeight {}

/// Two-slot anchored layout for the mobile action bar.
///
/// Layout: vertical flex. `children[0]` gets `flex_1` + `min_h_0` so it
/// consumes the remaining space (same idiom as `scrollable_list_wrapper`);
/// `children[1]` sits at its intrinsic height at the bottom of the content
/// area.
///
/// The dock slot renders only while the soft keyboard is up — the bar is
/// docked above the soft keyboard, so with the keyboard down there is nothing
/// to dock above and the main slot gets the whole box. The inset itself is
/// applied by the page container alone (`HolonApp::render`'s `.pb(...)`);
/// re-applying it here would push the bar a keyboard's height off-screen.
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let children = &node.children;
    assert_eq!(
        children.len(),
        2,
        "bottom_dock requires exactly 2 children (main, dock); got {}",
        children.len()
    );
    let main = super::render(&children[0], ctx);
    let keyboard_up =
        ctx.with_gpui(|_window, cx| cx.try_global::<KeyboardHeight>().is_some_and(|k| k.0 > 0.0));

    let mut root = div().size_full().flex().flex_col().child(
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .child(main),
    );
    if !keyboard_up {
        return root;
    }

    root = root.child(
        // `id` + `overflow_x_scroll`: the bar carries one op_button per
        // action-bar-exposed op, across both tiers, which can outrun a phone
        // width, so it scrolls sideways instead of wrapping or clipping.
        div()
            .id("bottom-dock-scroll")
            .w_full()
            .flex_shrink_0()
            .overflow_x_scroll()
            .child(render_dock_slot(&children[1], ctx)),
    );
    root
}

/// Render the dock slot at CONTENT height.
///
/// The slot has no definite height to measure against, so a collection taking
/// the default path — a `ReactiveShell` under `scrollable_list_wrapper`'s
/// `size_full` chain — resolves every button to a 0px box. Collections here
/// render eagerly instead, the same firewall content-sized columns use. The
/// walk exists because the slot holds the bar's tiers side by side (a `row` of
/// one collection per tier), so the collections are grandchildren, not the slot
/// itself.
fn render_dock_slot(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> gpui::AnyElement {
    if let Some(view) = node.collection.as_ref() {
        return super::column::eager_collection_div(view, ctx).into_any_element();
    }
    if node.children.iter().any(|c| c.collection.is_some()) {
        let mut row = div().flex().flex_row().items_center();
        for child in &node.children {
            row = row.child(render_dock_slot(child, ctx));
        }
        return row.into_any_element();
    }
    super::render(node, ctx)
}
