use super::prelude::*;
use holon_frontend::reactive_view_model::ReactiveViewKind;
use holon_frontend::vms_button_id_for;
use holon_frontend::ReactiveViewModel;

use crate::geometry::TransparentTracker;

struct ModeDesc {
    name: String,
    icon: String,
}

fn parse_modes(json: &str) -> Vec<ModeDesc> {
    let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return vec![];
    };
    arr.into_iter()
        .filter_map(|v| {
            let name = v.get("name")?.as_str()?.to_string();
            let icon = v.get("icon")?.as_str()?.to_string();
            Some(ModeDesc { name, icon })
        })
        .collect()
}

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let ReactiveViewKind::ViewModeSwitcher {
        entity_uri,
        modes,
        slot,
    } = &node.kind
    else {
        unreachable!()
    };

    let slot_content = slot.content.lock_ref().clone();
    let child_el = super::render(&slot_content, ctx);

    let mode_list = parse_modes(modes);
    if mode_list.is_empty() {
        return child_el;
    }

    let current_mode = ctx
        .services()
        .ui_state(entity_uri)
        .get("view_mode")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());
    // Default to the last mode (lowest priority = unconditional fallback)
    let default_mode = mode_list.last().map(|m| m.name.clone());
    let active = current_mode
        .as_deref()
        .or(default_mode.as_deref())
        .unwrap_or("tree");

    let icon_size = 14.0;
    let mut icons_row = div().flex().items_center().gap(px(2.0));

    for mode in &mode_list {
        let is_active = mode.name == active;
        // Canonical id that tests look up via BoundsRegistry to dispatch
        // a real click at the button's center.
        let tracked_id = vms_button_id_for(&entity_uri.to_string(), &mode.name);
        // GPUI's own ElementId for click hit-testing (different namespace
        // from BoundsRegistry's el_id — GPUI needs a hashable ElementId).
        let gpui_el_id = format!("vms-{}-{}", entity_uri.id(), mode.name);

        let services = ctx.services.clone();
        let mode_for_click = mode.name.clone();
        let click_key = entity_uri.clone();

        let icon_el = super::icon::render(&mode.icon, &icon_size, ctx);

        let button = div()
            .id(hashed_id(&gpui_el_id))
            .cursor_pointer()
            .p(px(2.0))
            .rounded(px(3.0))
            .when(is_active, |s| {
                s.bg(tc(ctx, |t| t.accent).opacity(0.15))
            })
            .when(!is_active, |s| {
                s.opacity(0.0).hover(|h| h.opacity(1.0))
            })
            .child(icon_el)
            .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                services.set_view_mode(&click_key, mode_for_click.clone());
            });

        let tracked_button = TransparentTracker::new(
            tracked_id,
            "vms_button",
            ctx.bounds_registry.clone(),
            button.into_any_element(),
        );

        icons_row = icons_row.child(tracked_button);
    }

    // Switcher icons float in the top-right corner as an absolute overlay;
    // the child content takes the full VMS box below them.
    let switcher_bar = div()
        .absolute()
        .top_0()
        .right_0()
        .pr(px(4.0))
        .pt(px(2.0))
        .child(icons_row);

    // VMS outer is an explicit flex_col so the slot wrapper gets a
    // definite flex-allocated height, and the slot wrapper uses the
    // same `flex_1 relative → absolute size_full` trick as
    // `columns::panel_wrap` to give the inner `AnyView::from(entity)`
    // (the `ReactiveShell`) a concrete box across the entity boundary.
    //
    // Plain `div().size_full().flex_1().relative()` without a flex
    // modifier relied on percentage sizing cascading through an
    // `AnyView` to resolve, which silently collapsed in production
    // (blank main panel) while passing in the fixture. The layout
    // proptest's nested-reactive arm now catches the same collapse via
    // `list[N] → list[M]`.
    let slot_wrapper = div().flex_1().relative().child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(child_el),
    );

    div()
        .size_full()
        .flex()
        .flex_col()
        .relative()
        .child(slot_wrapper)
        .child(switcher_bar)
        .into_any_element()
}
