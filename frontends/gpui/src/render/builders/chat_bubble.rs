use super::prelude::*;

const TEXT_PRIMARY: u32 = 0xE8E6E1FF;
const TEXT_SECONDARY: u32 = 0x9D9D95FF;
const BORDER_SUBTLE: u32 = 0x3A3A36FF;
const USER_BUBBLE: u32 = 0x2A3A3AFF;
const ASSISTANT_BUBBLE: u32 = 0x2A2A28FF;
const ACCENT_TEAL: u32 = 0x2A7D7DFF;
const ACCENT_TEAL_DIM: u32 = 0x1A4D4DFF;

fn c(hex: u32) -> Hsla {
    gpui::rgba(hex).into()
}

pub fn render(node: &holon_frontend::ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let sender = node.prop_str("sender").unwrap_or_else(|| "".to_string());
    let time = node.prop_str("time").unwrap_or_else(|| "".to_string());
    let children = &node.children;
    let child_elements = render_children(children, ctx);

    match sender.as_str() {
        "user" => render_user(&time, child_elements),
        "assistant" => render_assistant(&time, child_elements),
        "system" => render_system(child_elements),
        _ => render_assistant(&time, child_elements),
    }
}

fn render_user(time: &str, children: Vec<AnyElement>) -> Div {
    let mut bubble = div()
        .max_w(px(480.0))
        .ml_auto()
        .px(px(14.0))
        .py(px(10.0))
        .rounded_tl(px(16.0))
        .rounded_tr(px(4.0))
        .rounded_bl(px(16.0))
        .rounded_br(px(16.0))
        .bg(c(USER_BUBBLE))
        .border_1()
        .border_color(c(BORDER_SUBTLE))
        .text_size(px(14.0))
        .text_color(c(TEXT_PRIMARY));

    for child in children {
        bubble = bubble.child(child);
    }

    div()
        .w_full()
        .flex_shrink_0()
        .py(px(4.0))
        .child(bubble)
        .child(
            div()
                .flex()
                .justify_end()
                .pt(px(4.0))
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(c(TEXT_SECONDARY))
                        .px(px(4.0))
                        .child(time.to_string()),
                ),
        )
}

fn render_assistant(time: &str, children: Vec<AnyElement>) -> Div {
    let mut bubble = div()
        .max_w(px(520.0))
        .px(px(14.0))
        .py(px(10.0))
        .rounded_tl(px(4.0))
        .rounded_tr(px(16.0))
        .rounded_bl(px(16.0))
        .rounded_br(px(16.0))
        .bg(c(ASSISTANT_BUBBLE))
        .border_1()
        .border_color(c(BORDER_SUBTLE))
        .text_size(px(14.0))
        .text_color(c(TEXT_PRIMARY));

    for child in children {
        bubble = bubble.child(child);
    }

    div()
        .w_full()
        .flex_shrink_0()
        .py(px(4.0))
        .pl(px(38.0))
        .relative()
        .child(
            // Avatar
            div()
                .absolute()
                .left(px(0.0))
                .top(px(4.0))
                .w(px(28.0))
                .h(px(28.0))
                .rounded(px(14.0))
                .bg(c(ACCENT_TEAL_DIM))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(c(ACCENT_TEAL))
                        .child("H"),
                ),
        )
        .child(bubble)
        .child(
            div()
                .text_size(px(10.0))
                .text_color(c(TEXT_SECONDARY))
                .px(px(4.0))
                .pt(px(4.0))
                .child(time.to_string()),
        )
}

fn render_system(children: Vec<AnyElement>) -> Div {
    let mut inner = div()
        .text_size(px(11.0))
        .text_color(c(TEXT_SECONDARY));

    for child in children {
        inner = inner.child(child);
    }

    div()
        .w_full()
        .flex_shrink_0()
        .flex()
        .justify_center()
        .py(px(8.0))
        .child(inner)
}
