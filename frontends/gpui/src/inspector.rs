use gpui::prelude::FluentBuilder;
use gpui::*;

/// Register the inspector renderer and element inspectors on the GPUI app.
///
/// Call once during app init (before opening windows). The renderer draws a
/// side panel showing the selected element's source location, bounds, and
/// content size. Picking mode is activated via the toolbar button or by
/// clicking the magnifying-glass icon inside the panel.
pub fn init(cx: &mut App) {
    cx.set_inspector_renderer(Box::new(render_inspector));

    cx.register_inspector_element(
        |id: InspectorElementId, state: &DivInspectorState, _window: &mut Window, _cx: &mut App| {
            render_div_inspector(&id, state)
        },
    );
}

fn render_inspector(
    inspector: &mut Inspector,
    window: &mut Window,
    cx: &mut Context<Inspector>,
) -> AnyElement {
    let inspector_id = inspector.active_element_id().cloned();
    let is_picking = inspector.is_picking();

    let mut panel = div()
        .size_full()
        .bg(gpui::rgba(0x1e1e2eff))
        .text_color(gpui::rgba(0xccccccff))
        .text_size(px(12.0))
        .border_l_1()
        .border_color(gpui::rgba(0x333333ff))
        .flex()
        .flex_col()
        .child(
            // Toolbar
            div()
                .id("inspector-toolbar")
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .h(px(32.0))
                .px(px(8.0))
                .border_b_1()
                .border_color(gpui::rgba(0x333333ff))
                .child(
                    div()
                        .id("inspector-pick-btn")
                        .cursor_pointer()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .when(is_picking, |d| d.bg(gpui::rgba(0x3b82f660)))
                        .hover(|s| s.bg(gpui::rgba(0xffffff10)))
                        .child("🔍")
                        .on_mouse_down(MouseButton::Left, {
                            cx.listener(|inspector, _, window, _cx| {
                                inspector.start_picking();
                                window.refresh();
                            })
                        }),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(gpui::rgba(0x888888ff))
                        .child("INSPECTOR"),
                ),
        );

    if let Some(ref id) = inspector_id {
        panel = panel.child(render_element_id(id));
    }

    // Registered inspector states (DivInspectorState, etc.)
    let state_elements = inspector.render_inspector_states(window, cx);
    for el in state_elements {
        panel = panel.child(el);
    }

    if inspector_id.is_none() {
        panel = panel.child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(gpui::rgba(0x555555ff))
                .text_size(px(11.0))
                .child("Hover to inspect elements"),
        );
    }

    panel.into_any_element()
}

fn render_element_id(id: &InspectorElementId) -> Div {
    let loc = id.path.source_location;
    let loc_string = loc.to_string();
    // Strip common workspace prefix for readability
    let display = loc_string
        .strip_prefix("/Users/martin/Workspaces/pkm/holon/")
        .unwrap_or(&loc_string);

    div()
        .px(px(8.0))
        .py(px(6.0))
        .border_b_1()
        .border_color(gpui::rgba(0x333333ff))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(gpui::rgba(0x888888ff))
                .child("SOURCE"),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(gpui::rgba(0x7cacf8ff))
                .child(display.to_string()),
        )
}

fn render_div_inspector(id: &InspectorElementId, state: &DivInspectorState) -> Div {
    let bounds = state.bounds;
    let content = state.content_size;

    div()
        .px(px(8.0))
        .py(px(6.0))
        .border_b_1()
        .border_color(gpui::rgba(0x333333ff))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(section_header("BOUNDS"))
        .child(kv_row(
            "origin",
            format!(
                "{:.0}, {:.0}",
                bounds.origin.x.as_f32(),
                bounds.origin.y.as_f32()
            ),
        ))
        .child(kv_row(
            "size",
            format!(
                "{:.0} \u{00d7} {:.0}",
                bounds.size.width.as_f32(),
                bounds.size.height.as_f32()
            ),
        ))
        .child(section_header("CONTENT"))
        .child(kv_row(
            "size",
            format!(
                "{:.0} \u{00d7} {:.0}",
                content.width.as_f32(),
                content.height.as_f32()
            ),
        ))
        .child(section_header("ID"))
        .child(
            div()
                .text_size(px(10.0))
                .text_color(gpui::rgba(0x999999ff))
                .child(format!("instance {}", id.instance_id,)),
        )
}

fn section_header(label: &str) -> Div {
    div()
        .text_size(px(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui::rgba(0x888888ff))
        .mt(px(4.0))
        .child(label.to_string())
}

fn kv_row(key: &str, value: String) -> Div {
    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(11.0))
                .text_color(gpui::rgba(0x777777ff))
                .w(px(50.0))
                .child(key.to_string()),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(gpui::rgba(0xccccccff))
                .child(value),
        )
}
