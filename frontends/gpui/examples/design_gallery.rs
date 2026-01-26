//! Standalone design gallery — realistic app mockups for VISION_UI.md.
//!
//! Run with:
//!   cargo run --example design_gallery
//!
//! Shows Orient, Flow, Capture, and Chat modes with interactive tab switching.
//! No database, no DI, no backend.
//!
//! MCP server runs on port 8523 (override with MCP_SERVER_PORT env var)
//! so it can run alongside the real Holon app (port 8520).

use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;

#[cfg(feature = "hot-reload")]
use subsecond;

const DESIGN_GALLERY_MCP_PORT: u16 = 8523;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,holon_mcp=info".into()),
        )
        .init();

    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let _guard = runtime.enter();

    let stub_services: Arc<dyn holon_frontend::reactive::BuilderServices> =
        Arc::new(holon_frontend::StubBuilderServices::new());

    let debug = Arc::new(holon_mcp::server::DebugServices::default());

    holon_mcp::di::start_embedded_mcp_server_with_debug(
        None,
        Some(stub_services.clone()),
        DESIGN_GALLERY_MCP_PORT,
        debug.clone(),
    );

    let app = Application::with_platform(gpui_platform::current_platform(false));
    app.run(move |cx| {
        gpui_component::init(cx);
        gpui_component::theme::Theme::change(gpui_component::theme::ThemeMode::Dark, None, cx);

        let window_options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Holon".into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::point(px(14.0), px(14.0))),
            }),
            window_background: WindowBackgroundAppearance::Opaque,
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1100.0), px(750.0)),
                cx,
            ))),
            ..Default::default()
        };

        let debug_for_window = debug.clone();
        cx.spawn(async move |cx| {
            let window_handle = cx.open_window(window_options, |window, cx| {
                window.on_window_should_close(cx, |_window, cx| {
                    cx.quit();
                    true
                });
                let svc = stub_services.clone();
                let view = cx.new(|cx| GalleryView::new_with_services(svc, cx));
                let any_view: AnyView = view.into();
                cx.new(|cx| gpui_component::Root::new(any_view, window, cx))
            })?;

            // design_gallery doesn't need MCP interaction pump — skip it

            Ok::<_, anyhow::Error>(())
        })
        .detach();

        cx.activate(true);
    });
}

// ── Colors from VISION_UI.md (dark theme) ────────────────────────────────

const BG: u32 = 0x1A1A18FF;
const SURFACE: u32 = 0x252522FF;
const TEXT_PRIMARY: u32 = 0xE8E6E1FF;
const TEXT_SECONDARY: u32 = 0x9D9D95FF;
const SIDEBAR_BG: u32 = 0x1E1E1CFF;
const BORDER_SUBTLE: u32 = 0x3A3A36FF;

fn c(hex: u32) -> Hsla {
    gpui::rgba(hex).into()
}

// ── Mode ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Capture,
    Orient,
    Flow,
    Chat,
    Board,
}

impl Mode {
    fn label(&self) -> &'static str {
        match self {
            Mode::Capture => "Capture",
            Mode::Orient => "Orient",
            Mode::Flow => "Flow",
            Mode::Chat => "Chat",
            Mode::Board => "Board",
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            Mode::Capture => "↓",
            Mode::Orient => "⊹",
            Mode::Flow => "≡",
            Mode::Chat => "◎",
            Mode::Board => "▦",
        }
    }
}

// ── Gallery View (stateful) ──────────────────────────────────────────────

struct GalleryView {
    mode: Mode,
    stub_services: Arc<dyn holon_frontend::reactive::BuilderServices>,
    bounds_registry: holon_gpui::geometry::BoundsRegistry,
    entity_cache: holon_gpui::entity_view_registry::EntityCache,
}

impl Render for GalleryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        div()
            .id("gallery-root")
            .size_full()
            .bg(c(BG))
            .text_color(c(TEXT_PRIMARY))
            .flex()
            .flex_col()
            .child(self.top_bar(cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .child(sidebar())
                    .child({
                        let expr = match mode {
                            Mode::Orient => holon_frontend::widget_gallery::orient_mode_expr(),
                            Mode::Flow => holon_frontend::widget_gallery::flow_mode_expr(),
                            Mode::Capture => holon_frontend::widget_gallery::capture_mode_expr(),
                            Mode::Chat => holon_frontend::widget_gallery::chat_mode_expr(),
                            Mode::Board => holon_frontend::widget_gallery::board_mode_expr(),
                        };
                        let rvm = holon_frontend::widget_gallery::mode_view_model(&expr);
                        let gpui_ctx = holon_gpui::render::builders::GpuiRenderContext::new(
                            holon_frontend::RenderContext::default(),
                            self.stub_services.clone(),
                            self.bounds_registry.clone(),
                            holon_gpui::entity_view_registry::LocalEntityScope::new()
                                .with_cache(self.entity_cache.clone()),
                            holon_gpui::navigation_state::NavigationState::new(),
                            window,
                            cx,
                        );
                        let content_el = holon_gpui::render::builders::render(&rvm, &gpui_ctx);
                        let mut content_div = div()
                            .id("content-area")
                            .flex_1()
                            .p(px(24.0))
                            .overflow_y_scroll();
                        if matches!(mode, Mode::Flow | Mode::Capture | Mode::Chat) {
                            content_div = content_div
                                .flex()
                                .flex_col()
                                .items_center()
                                .child(div().w_full().max_w(px(640.0)).child(content_el));
                        } else {
                            content_div = content_div.child(content_el);
                        }
                        content_div
                    }),
            )
    }
}

impl GalleryView {
    fn top_bar(&self, cx: &mut Context<Self>) -> Div {
        div()
            .w_full()
            .h(px(44.0))
            .flex()
            .flex_row()
            .items_center()
            .px(px(80.0))
            .bg(c(SIDEBAR_BG))
            .border_b_1()
            .border_color(c(BORDER_SUBTLE))
            .child(self.mode_switcher(cx))
            .child(
                div().flex_1().flex().justify_center().child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c(TEXT_SECONDARY))
                        .child("Holon"),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(12.0))
                    .child(top_bar_icon("⌕"))
                    .child(top_bar_icon("⚙")),
            )
    }

    fn mode_switcher(&self, cx: &mut Context<Self>) -> Div {
        let modes = [
            Mode::Capture,
            Mode::Orient,
            Mode::Flow,
            Mode::Chat,
            Mode::Board,
        ];
        let mut row = div()
            .flex()
            .gap(px(2.0))
            .bg(c(0x16161400))
            .rounded(px(8.0))
            .p(px(3.0));
        for m in modes {
            row = row.child(self.mode_tab(m, cx));
        }
        row
    }

    fn mode_tab(&self, target: Mode, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.mode == target;
        let label = target.label();
        let icon = target.icon();
        let base = div()
            .id(ElementId::Name(label.into()))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(2.0))
            .px(px(14.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.mode = target;
                    cx.notify();
                }),
            )
            .child(div().text_size(px(14.0)).child(icon))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label),
            );
        if active {
            base.bg(c(SURFACE)).text_color(c(TEXT_PRIMARY))
        } else {
            base.text_color(c(TEXT_SECONDARY))
                .hover(|s| s.text_color(c(TEXT_PRIMARY)).bg(c(0x22221FFF)))
        }
    }
}

impl GalleryView {
    fn new_with_services(
        services: Arc<dyn holon_frontend::reactive::BuilderServices>,
        cx: &mut Context<Self>,
    ) -> Self {
        let bounds_registry = holon_gpui::geometry::BoundsRegistry::new();
        Self {
            mode: Mode::Chat,
            stub_services: services,
            bounds_registry,
            entity_cache: Default::default(),
        }
    }
}

fn top_bar_icon(symbol: &str) -> Div {
    let symbol = symbol.to_string();
    div()
        .text_size(px(15.0))
        .text_color(c(TEXT_SECONDARY))
        .cursor_pointer()
        .hover(|s| s.text_color(c(TEXT_PRIMARY)))
        .child(symbol)
}

// ── Sidebar (shared) ─────────────────────────────────────────────────────

fn sidebar() -> impl IntoElement {
    div()
        .id("sidebar")
        .w(px(180.0))
        .flex_shrink_0()
        .bg(c(SIDEBAR_BG))
        .border_r_1()
        .border_color(c(BORDER_SUBTLE))
        .overflow_y_scroll()
        .py(px(12.0))
        .px(px(12.0))
        .flex()
        .flex_col()
        .gap(px(20.0))
        .child(sidebar_section(
            "Projects",
            &["Projects", "Implementation", "Resources", "Projects"],
        ))
        .child(sidebar_section(
            "Areas",
            &["Delta Sharing", "Implementation"],
        ))
        .child(sidebar_section("Resources", &["Projects", "Resources"]))
        .child(sidebar_section("Archives", &["Archives", "Learnings"]))
}

fn sidebar_section(title: &str, items: &[&str]) -> Div {
    let mut section = div().flex().flex_col().gap(px(2.0)).child(
        div()
            .text_size(px(11.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c(TEXT_SECONDARY))
            .pb(px(4.0))
            .child(title.to_uppercase()),
    );
    for item in items {
        section = section.child(sidebar_item(item));
    }
    section
}

fn sidebar_item(label: &str) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .px(px(6.0))
        .py(px(3.0))
        .rounded(px(4.0))
        .text_size(px(13.0))
        .text_color(c(TEXT_PRIMARY))
        .cursor_pointer()
        .hover(|s| s.bg(c(SURFACE)))
        .child(
            div()
                .text_size(px(11.0))
                .text_color(c(TEXT_SECONDARY))
                .child("▸"),
        )
        .child(label.to_string())
}

// All mode content rendered via builder pipeline.
// See holon_frontend::widget_gallery::{orient_mode_expr, flow_mode_expr, capture_mode_expr, chat_mode_expr}
