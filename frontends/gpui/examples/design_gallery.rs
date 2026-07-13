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

        cx.spawn(async move |cx| {
            cx.open_window(window_options, |window, cx| {
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

// ── Warm charcoal palette (dark theme, per Holon Desk reference) ─────────
//
// The base is a warm charcoal/brown, not neutral gray. The time axis runs
// cool/dark on the left (past) to warm/amber-tinted on the right (future).
// The focused card inverts to ivory/cream with a dark serif title and an
// amber halo (box-shadow glow).

const BG: u32 = 0x1A1A18FF;
const SURFACE: u32 = 0x252522FF;
const TEXT_PRIMARY: u32 = 0xE8E6E1FF;
const TEXT_SECONDARY: u32 = 0x9D9D95FF;
const SIDEBAR_BG: u32 = 0x1E1E1CFF;
const BORDER_SUBTLE: u32 = 0x3A3A36FF;

// Warm charcoal-brown base for the desk surface.
const DESK_BASE: u32 = 0x1B1814FF;
// Time-axis gradient: cool/dark at the wake (past) edge → warm amber-tinted
// at the shore (future) edge. Subtle — communicates direction without
// shouting.
const AXIS_FROM: u32 = 0x161311FF;
const AXIS_TO: u32 = 0x241D14FF;

// Zone panel colors. Wake is the coolest/dimmest (history). Shore is a
// rounded container slightly lighter/warmer than the base. Center is the
// open present.
const WAKE_BG: u32 = 0x151210FF;
const CENTER_BG: u32 = 0x1B1714FF;
const SHORE_BG: u32 = 0x221C16FF;

// Focused card: ivory/cream surface with dark serif text.
const IVORY: u32 = 0xF2EBDDFF;
const IVORY_TEXT: u32 = 0x2A2418FF;

// Amber-gold glow + accents.
const AMBER: u32 = 0xD4A373FF;
const AMBER_DIM: u32 = 0x8A6A45FF;
const ACCENT_TASK: u32 = 0x6DBDBDFF;
const ACCENT_JOURNAL: u32 = 0x8A8A82FF;
const ACCENT_PINNED: u32 = 0xD4A373FF;
const ACCENT_ARRIVAL: u32 = 0x7D9D7DFF;

const SERIF: &str = "Georgia";

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
    Actions,
    Desk,
}

impl Mode {
    fn label(&self) -> &'static str {
        match self {
            Mode::Capture => "Capture",
            Mode::Orient => "Orient",
            Mode::Flow => "Flow",
            Mode::Chat => "Chat",
            Mode::Board => "Board",
            Mode::Actions => "Actions",
            Mode::Desk => "Desk",
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            Mode::Capture => "↓",
            Mode::Orient => "⊹",
            Mode::Flow => "≡",
            Mode::Chat => "◎",
            Mode::Board => "▦",
            Mode::Actions => "⚙",
            Mode::Desk => "⬒",
        }
    }
}

// ── Gallery View (stateful) ──────────────────────────────────────────────

struct GalleryView {
    mode: Mode,
    stub_services: Arc<dyn holon_frontend::reactive::BuilderServices>,
    bounds_registry: holon_gpui::geometry::BoundsRegistry,
    entity_cache: holon_gpui::entity_view_registry::EntityCache,
    /// See `view_cache`.
    view_cache: Option<(
        Mode,
        Arc<holon_frontend::reactive_view_model::ReactiveViewModel>,
    )>,
    /// Desk-mode focus settle toggle. When true, chrome retracts and non-
    /// centered cards dim further. Toggled by clicking the centered card.
    focus_settled: bool,
}

impl Render for GalleryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        let is_desk = matches!(mode, Mode::Desk);
        let show_sidebar = !is_desk || !self.focus_settled;

        let sidebar_el: AnyElement = if show_sidebar {
            sidebar().into_any_element()
        } else {
            div().into_any_element()
        };

        let content: AnyElement = if is_desk {
            self.render_desk_surface(cx).into_any_element()
        } else {
            let rvm = self.view_model_for(mode);
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
            content_div.into_any_element()
        };

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
                    .child(sidebar_el)
                    .child(content),
            )
    }
}

impl GalleryView {
    /// Return the interpreted tree for `mode`, reusing the cached one so
    /// per-node view state (hover/expand `Mutable`s) survives repaints.
    /// Rebuilt only when the mode changes.
    fn view_model_for(
        &mut self,
        mode: Mode,
    ) -> Arc<holon_frontend::reactive_view_model::ReactiveViewModel> {
        if let Some((cached_mode, rvm)) = &self.view_cache {
            if *cached_mode == mode {
                return rvm.clone();
            }
        }
        let expr = match mode {
            Mode::Orient => holon_frontend::widget_gallery::orient_mode_expr(),
            Mode::Flow => holon_frontend::widget_gallery::flow_mode_expr(),
            Mode::Capture => holon_frontend::widget_gallery::capture_mode_expr(),
            Mode::Chat => holon_frontend::widget_gallery::chat_mode_expr(),
            Mode::Board => holon_frontend::widget_gallery::board_mode_expr(),
            Mode::Actions => holon_frontend::widget_gallery::actions_mode_expr(),
            Mode::Desk => {
                unreachable!("Desk mode renders directly in GPUI, not via builder pipeline")
            }
        };
        let rvm = Arc::new(holon_frontend::widget_gallery::mode_view_model(&expr));
        self.view_cache = Some((mode, rvm.clone()));
        rvm
    }

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
            Mode::Desk,
            Mode::Capture,
            Mode::Orient,
            Mode::Flow,
            Mode::Chat,
            Mode::Board,
            Mode::Actions,
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
        _: &mut Context<Self>,
    ) -> Self {
        let bounds_registry = holon_gpui::geometry::BoundsRegistry::new();
        Self {
            mode: Mode::Desk,
            stub_services: services,
            bounds_registry,
            entity_cache: Default::default(),
            view_cache: None,
            focus_settled: false,
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
            &["Holon", "Delta Sharing", "Website"],
        ))
        .child(sidebar_section("Areas", &["Health", "Finances"]))
        .child(sidebar_section("Resources", &["Rust", "Design Systems"]))
        .child(sidebar_section("Archives", &["2025", "Old Projects"]))
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

// ── Desk Mode: Attention Environment Mockup ──────────────────────────────
//
// Desk mode is rendered directly in GPUI (not through the builder pipeline)
// because the render expression vocabulary lacks free 2D placement, opacity
// effects, and stateful click-to-toggle. The real architecture will drive
// zone layout from data through the builder pipeline; this mockup
// demonstrates the visual concept with mock data.
//
// DESIGN (from ratified ideation): A single bounded surface, no scrolling.
// Time runs horizontally — past exits LEFT, future arrives RIGHT. Vertical
// axis is free/user territory. Zones are DATA (structs + slices), never
// hardcoded layout.

// ── Desk Data Structures ────────────────────────────────────────────────
//
// Zones, cards, and per-card offsets are DATA. The layout code never
// hardcodes a position — it reads `cell` (grid cell) and `jitter`
// (deterministic ±px offset) from the card structs.

#[derive(Debug, Clone, Copy)]
enum CardAge {
    Fresh,
    Aging,
    Old,
}

impl CardAge {
    /// Human-readable age caption, e.g. "(3 days old)".
    fn caption(self) -> &'static str {
        match self {
            CardAge::Fresh => "(just now)",
            CardAge::Aging => "(2 days old)",
            CardAge::Old => "(last week)",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CardKind {
    Task,
    Journal,
    Pinned,
    Arrival,
}

impl CardKind {
    fn accent(self) -> Hsla {
        match self {
            CardKind::Task => c(ACCENT_TASK),
            CardKind::Journal => c(ACCENT_JOURNAL),
            CardKind::Pinned => c(ACCENT_PINNED),
            CardKind::Arrival => c(ACCENT_ARRIVAL),
        }
    }

    /// Short chip label shown at the top of a card.
    fn chip(self) -> &'static str {
        match self {
            CardKind::Task => "Task",
            CardKind::Journal => "Journal",
            CardKind::Pinned => "Note",
            CardKind::Arrival => "Arrival",
        }
    }
}

struct DeskCard {
    title: &'static str,
    subtitle: &'static str,
    age: CardAge,
    kind: CardKind,
    /// This card is the focused / active anchor.
    focused: bool,
    /// Grid cell on a 3-column layout within the center zone:
    /// (col 0..3, row 0..3).
    cell: (u32, u32),
    /// Deterministic per-card offset in px (applied to the cell anchor).
    jitter: (f32, f32),
}

struct DeskZone {
    width: f32,
    bg: u32,
}

// ── Desk Rendering ─────────────────────────────────────────────────────

impl GalleryView {
    fn render_desk_surface(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.focus_settled;

        let wake_zone = DeskZone {
            width: 170.0,
            bg: WAKE_BG,
        };
        let shore_zone = DeskZone {
            width: 200.0,
            bg: SHORE_BG,
        };

        // Wake (left) — "History" panel: a vertical list of small time-stamped
        // journal entries, dimmer with age.
        let wake_entries = &[
            DeskCard {
                title: "Tiny journal entry — captured a thought on attention",
                subtitle: "7:50 AM",
                age: CardAge::Fresh,
                kind: CardKind::Journal,
                focused: false,
                cell: (0, 0),
                jitter: (0.0, 0.0),
            },
            DeskCard {
                title: "Returned: read Loro CRDT paper",
                subtitle: "yesterday",
                age: CardAge::Aging,
                kind: CardKind::Journal,
                focused: false,
                cell: (0, 0),
                jitter: (0.0, 0.0),
            },
            DeskCard {
                title: "Merged PR #40 — DiffEvent incremental projection",
                subtitle: "Jul 12",
                age: CardAge::Old,
                kind: CardKind::Journal,
                focused: false,
                cell: (0, 0),
                jitter: (0.0, 0.0),
            },
            DeskCard {
                title: "Shipped undo fix for SetEdgeField",
                subtitle: "Jul 10",
                age: CardAge::Old,
                kind: CardKind::Journal,
                focused: false,
                cell: (0, 0),
                jitter: (0.0, 0.0),
            },
        ];

        // Center (present) — focused card + neighbors.
        let center_cards = &[
            DeskCard {
                title: "Fix SetEdgeField undo",
                subtitle: "DOING · P1",
                age: CardAge::Fresh,
                kind: CardKind::Task,
                focused: false,
                cell: (0, 0),
                jitter: (8.0, 6.0),
            },
            DeskCard {
                title: "Review Fable PR",
                subtitle: "TODO · P1",
                age: CardAge::Fresh,
                kind: CardKind::Task,
                focused: false,
                cell: (2, 0),
                jitter: (-10.0, 10.0),
            },
            DeskCard {
                title: "Draft ADR 0025",
                subtitle: "TODO · P2",
                age: CardAge::Aging,
                kind: CardKind::Task,
                focused: false,
                cell: (0, 2),
                jitter: (12.0, -6.0),
            },
            DeskCard {
                title: "Note: WIP-limits lesson",
                subtitle: "reference",
                age: CardAge::Aging,
                kind: CardKind::Pinned,
                focused: false,
                cell: (2, 2),
                jitter: (-8.0, 8.0),
            },
            // The focused anchor, center-stage.
            DeskCard {
                title: "Attention Environment architecture",
                subtitle: "DOING · P0",
                age: CardAge::Fresh,
                kind: CardKind::Task,
                focused: true,
                cell: (1, 1),
                jitter: (0.0, 0.0),
            },
        ];

        // Shore (right) — "Arrival" panel: calendar fixture + 3 arrival cards
        // + ornament + dashed empty slot.
        let shore_cards = &[
            DeskCard {
                title: "Calendar prep",
                subtitle: "9:30 AM · standup",
                age: CardAge::Fresh,
                kind: CardKind::Arrival,
                focused: false,
                cell: (0, 0),
                jitter: (0.0, 0.0),
            },
            DeskCard {
                title: "Inbox capture",
                subtitle: "2 new",
                age: CardAge::Fresh,
                kind: CardKind::Arrival,
                focused: false,
                cell: (0, 0),
                jitter: (0.0, 0.0),
            },
            DeskCard {
                title: "Resurfaced note",
                subtitle: "WIP-limits",
                age: CardAge::Aging,
                kind: CardKind::Arrival,
                focused: false,
                cell: (0, 0),
                jitter: (0.0, 0.0),
            },
        ];

        let cal_entries: &'static [(&'static str, &'static str)] = &[
            ("9:30", "Standup"),
            ("11:00", "Design review"),
            ("2:00", "1:1"),
        ];

        let wake_w = px(wake_zone.width);
        let shore_w = px(shore_zone.width);

        // Time-axis gradient: cool/dark at the wake (past) edge → warm
        // amber-tinted at the shore (future) edge. Two-stop linear gradient
        // at 90° (left→right). Painted as the desk base; zone panels sit on
        // top.
        let axis = linear_gradient(
            90.0,
            linear_color_stop(c(AXIS_FROM), 0.0),
            linear_color_stop(c(AXIS_TO), 1.0),
        );

        // Wake panel.
        let wake = render_wake_panel(wake_entries, wake_zone, focus);

        // Center zone.
        let center = render_center_zone(center_cards, focus, wake_w, shore_w, cx);

        // Shore panel.
        let shore = render_shore_panel(shore_cards, cal_entries, shore_zone, focus);

        let mut surface = div()
            .id("desk-surface")
            .relative()
            .size_full()
            .bg(axis)
            .child(wake)
            .child(center)
            .child(shore);

        // Single muted hint line at the very bottom of the desk.
        surface = surface.child(
            div()
                .absolute()
                .bottom(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .h(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(c(DESK_BASE))
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(SERIF)
                        .text_color(c(0x6B5F4AFF))
                        .child(if focus {
                            "settled — click the glowing card to release focus"
                        } else {
                            "click the glowing card to settle into focus"
                        }),
                ),
        );

        surface
    }
}

fn render_wake_panel(entries: &'static [DeskCard], zone: DeskZone, focus: bool) -> Div {
    let mut panel = div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .w(px(zone.width))
        .h_full()
        .bg(c(zone.bg))
        .border_r_1()
        .border_color(c(0x2A2520FF))
        .flex()
        .flex_col()
        .p(px(16.0))
        .gap(px(10.0))
        .opacity(if focus { 0.3 } else { 1.0 })
        // Corner label: "PAST / THE WAKE" top-right of the panel.
        .child(
            div()
                .w_full()
                .flex()
                .justify_end()
                .text_size(px(9.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(0x5A4F40FF))
                .child("PAST · THE WAKE"),
        )
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(20.0))
                        .font_family(SERIF)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(0xC4B8A4FF))
                        .child("History"),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(c(0x7A6E5AFF))
                        .child("wake"),
                ),
        );

    // Entry list. Entries dim with age (freshest brightest at top).
    let mut list = div().w_full().flex().flex_col().gap(px(8.0)).flex_1();
    for (i, card) in entries.iter().enumerate() {
        let age_dim = match card.age {
            CardAge::Fresh => 1.0,
            CardAge::Aging => 0.72,
            CardAge::Old => 0.5,
        };
        list = list.child(wake_entry_render(card).opacity(if focus {
            age_dim * 0.6
        } else {
            age_dim
        }));
        let _ = i;
    }
    panel = panel.child(list);
    panel
}

fn wake_entry_render(card: &DeskCard) -> Div {
    let accent = card.kind.accent();
    div()
        .w_full()
        .relative()
        .flex()
        .flex_col()
        .gap(px(3.0))
        .py(px(6.0))
        .pl(px(10.0))
        .pr(px(2.0))
        .border_l_2()
        .border_color(accent.opacity(0.6))
        .child(
            div()
                .text_size(px(9.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(c(0x6B5F4AFF))
                .child(card.subtitle),
        )
        .child(
            div()
                .text_size(px(12.0))
                .font_family(SERIF)
                .text_color(c(0xB8AC98FF))
                .child(card.title),
        )
}

fn render_shore_panel(
    cards: &'static [DeskCard],
    cal: &'static [(&'static str, &'static str)],
    zone: DeskZone,
    focus: bool,
) -> Div {
    let mut panel = div()
        .absolute()
        .top(px(0.0))
        .right(px(0.0))
        .w(px(zone.width))
        .h_full()
        .flex()
        .flex_col()
        .p(px(16.0))
        .opacity(if focus { 0.3 } else { 1.0 });

    // Corner label: "FUTURE / ARRIVAL SHORE" (amber), top-right.
    panel = panel.child(
        div()
            .w_full()
            .flex()
            .justify_end()
            .text_size(px(9.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c(AMBER_DIM))
            .child("FUTURE · ARRIVAL SHORE ▸"),
    );

    // Rounded container panel, slightly lighter/warmer than the base.
    let mut container = div()
        .w_full()
        .flex_1()
        .flex()
        .flex_col()
        .bg(c(zone.bg))
        .rounded(px(14.0))
        .border_1()
        .border_color(c(0x3A2E20FF))
        .p(px(12.0))
        .gap(px(8.0))
        // Calendar fixture at top.
        .child(render_calendar_fixture(cal));

    for card in cards {
        container = container.child(shore_card_render(card));
    }

    // Small ornament divider.
    container = container.child(
        div()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .py(px(2.0))
            .child(div().w(px(20.0)).h(px(1.0)).bg(c(0x3A2E20FF)))
            .child(
                div()
                    .text_size(px(9.0))
                    .text_color(c(0x5A4F40FF))
                    .child("✦"),
            )
            .child(div().w(px(20.0)).h(px(1.0)).bg(c(0x3A2E20FF))),
    );

    // Dashed empty slot at the bottom.
    container = container.child(empty_shore_slot());

    panel = panel.child(container);

    // Bottom-pinned "N waiting" caption.
    panel = panel.child(
        div()
            .w_full()
            .text_right()
            .text_size(px(10.0))
            .font_family(SERIF)
            .text_color(c(0x7A6E5AFF))
            .child(format!("{} waiting", cards.len())),
    );

    panel
}

fn render_calendar_fixture(entries: &'static [(&'static str, &'static str)]) -> Div {
    let mut cal = div()
        .w_full()
        .flex()
        .flex_col()
        .bg(c(0x2A2118FF))
        .rounded(px(10.0))
        .border_1()
        .border_color(c(0x403020FF))
        .p(px(10.0))
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(div().text_size(px(14.0)).child("🕐"))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_family(SERIF)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(0xD4C4A8FF))
                        .child("Calendar"),
                )
                .child(
                    div()
                        .flex_1()
                        .text_right()
                        .text_size(px(9.0))
                        .text_color(c(0x6B5F4AFF))
                        .child("fixture"),
                ),
        );
    for (time, desc) in entries {
        cal = cal.child(
            div()
                .flex()
                .flex_row()
                .gap(px(8.0))
                .child(
                    div()
                        .w(px(40.0))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c(AMBER))
                        .child(*time),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(c(0xB8AC98FF))
                        .child(*desc),
                ),
        );
    }
    cal
}

fn render_center_zone(
    cards: &'static [DeskCard],
    focus: bool,
    wake_w: Pixels,
    shore_w: Pixels,
    cx: &mut Context<GalleryView>,
) -> Div {
    let header_h = px(64.0);
    let bottom_h = px(30.0);
    let side_pad = px(28.0);

    let mut zone = div()
        .absolute()
        .top(px(0.0))
        .left(wake_w)
        .right(shore_w)
        .h_full()
        .bg(c(CENTER_BG))
        .opacity(if focus { 1.0 } else { 1.0 })
        // Big serif display header, top-left of the center zone.
        .child(
            div()
                .absolute()
                .top(px(18.0))
                .left(side_pad)
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(28.0))
                        .font_family(SERIF)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(0xE8DCC4FF))
                        .child("DELIBERATE"),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(c(0x6B5F4AFF))
                        .child("a bounded surface for attention"),
                ),
        )
        // Amber corner label, top-right of the center zone.
        .child(
            div()
                .absolute()
                .top(px(24.0))
                .right(side_pad)
                .text_size(px(9.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(AMBER_DIM))
                .child("FUTURE ▸"),
        );

    let cols = 3u32;
    let rows = 3u32;

    let mut grid = div()
        .absolute()
        .top(header_h)
        .left(side_pad)
        .right(side_pad)
        .bottom(bottom_h)
        .id("desk-center-grid")
        .relative();

    // Optional tiny ghost tiles near the focused card — very low-alpha
    // decorative squares as in the reference.
    if !focus {
        for (gx, gy, gw, gh) in [
            (0.62f32, 0.30f32, 16.0f32, 16.0f32),
            (0.34, 0.62, 12.0, 12.0),
            (0.70, 0.66, 14.0, 14.0),
        ] {
            grid = grid.child(
                div()
                    .absolute()
                    .top(relative(gy))
                    .left(relative(gx))
                    .w(px(gw))
                    .h(px(gh))
                    .rounded(px(4.0))
                    .bg(c(AMBER).opacity(0.06)),
            );
        }
    }

    for card in cards {
        let (col, row) = card.cell;
        let (jx, jy) = card.jitter;
        if card.focused {
            // Focused card: anchored at grid center, offset by half its size so
            // its CENTER sits on the grid center.
            let focused_el = focused_card_render(card, focus, cx);
            grid = grid.child(
                focused_el
                    .absolute()
                    .top(relative(0.5))
                    .left(relative(0.5))
                    .mt(px(-70.0 + jy))
                    .ml(px(-150.0 + jx)),
            );
        } else {
            let left_frac = (col as f32) / (cols as f32);
            let top_frac = (row as f32) / (rows as f32);
            grid = grid.child(
                neighbor_card_render(card)
                    .absolute()
                    .top(relative(top_frac))
                    .left(relative(left_frac))
                    .mt(px(jy))
                    .ml(px(jx)),
            );
        }
    }

    zone = zone.child(grid);
    zone
}

/// The focused card: ivory/cream background, dark large serif multi-line
/// title, amber halo (box-shadow glow), colored left edge, kind chip.
fn focused_card_render(card: &DeskCard, focus: bool, cx: &mut Context<GalleryView>) -> Div {
    let accent = card.kind.accent();
    // Amber halo: a large soft glow centered on the card. Intensifies in
    // focus-settle mode.
    let glow_alpha = if focus { 0.55 } else { 0.32 };
    let halo = vec![BoxShadow {
        color: c(AMBER).opacity(glow_alpha),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(48.0),
        spread_radius: px(6.0),
    }];

    div()
        .w(px(300.0))
        .relative()
        .flex()
        .flex_col()
        .bg(c(IVORY))
        .rounded(px(10.0))
        .shadow(halo)
        .pl(px(18.0))
        .pr(px(16.0))
        .py(px(16.0))
        .gap(px(6.0))
        .overflow_hidden()
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.focus_settled = !this.focus_settled;
                cx.notify();
            }),
        )
        // Colored left edge (3px strip — GPUI's discrete border widths
        // don't offer a crisp 3px, so use an absolute child).
        .child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .bottom(px(0.0))
                .w(px(3.0))
                .bg(accent),
        )
        // Kind chip at the top: small colored square + label.
        .child(kind_chip(card.kind, true))
        .child(
            div()
                .mt(px(2.0))
                .text_size(px(18.0))
                .font_family(SERIF)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(IVORY_TEXT))
                .line_height(px(24.0))
                .child(card.title),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(c(0x6A5F48FF))
                .child(format!("{} · {}", card.subtitle, card.age.caption())),
        )
}

/// An unfocused neighbor card: dark, muted, serif title in gray, age
/// caption under the title, own kind chip.
fn neighbor_card_render(card: &DeskCard) -> Div {
    let accent = card.kind.accent();
    let title_color = match card.age {
        CardAge::Fresh => c(0xC4B8A4FF),
        CardAge::Aging => c(0x9A8E7AFF),
        CardAge::Old => c(0x7A6E5AFF),
    };
    div()
        .w(px(170.0))
        .relative()
        .flex()
        .flex_col()
        .bg(c(0x241E18FF))
        .rounded(px(8.0))
        .pl(px(12.0))
        .pr(px(10.0))
        .py(px(10.0))
        .gap(px(4.0))
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .bottom(px(0.0))
                .w(px(3.0))
                .bg(accent.opacity(0.8)),
        )
        .child(kind_chip(card.kind, false))
        .child(
            div()
                .mt(px(2.0))
                .text_size(px(14.0))
                .font_family(SERIF)
                .text_color(title_color)
                .line_height(px(19.0))
                .child(card.title),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(c(0x6B5F4AFF))
                .child(card.age.caption()),
        )
}

fn shore_card_render(card: &DeskCard) -> Div {
    let accent = card.kind.accent();
    let dim = match card.age {
        CardAge::Fresh => 1.0,
        CardAge::Aging => 0.7,
        CardAge::Old => 0.5,
    };
    div()
        .w_full()
        .relative()
        .flex()
        .flex_col()
        .bg(c(0x2A2118FF))
        .rounded(px(8.0))
        .pl(px(12.0))
        .pr(px(10.0))
        .py(px(9.0))
        .gap(px(3.0))
        .overflow_hidden()
        .opacity(dim)
        .child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .bottom(px(0.0))
                .w(px(3.0))
                .bg(accent),
        )
        .child(kind_chip(card.kind, false))
        .child(
            div()
                .mt(px(2.0))
                .text_size(px(13.0))
                .font_family(SERIF)
                .text_color(c(0xC4B8A4FF))
                .child(card.title),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(c(0x7A6E5AFF))
                .child(card.subtitle),
        )
}

/// A small kind chip: colored square + sans label. On the focused (ivory)
/// card the chip uses dark text; on dark cards it uses light text.
fn kind_chip(kind: CardKind, on_ivory: bool) -> Div {
    let accent = kind.accent();
    let label_color = if on_ivory {
        c(0x4A3F28FF)
    } else {
        c(0x9A8E7AFF)
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .child(div().w(px(8.0)).h(px(8.0)).rounded(px(2.0)).bg(accent))
        .child(
            div()
                .text_size(px(9.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(label_color)
                .child(kind.chip().to_uppercase()),
        )
}

fn empty_shore_slot() -> Div {
    div()
        .w_full()
        .h(px(46.0))
        .rounded(px(8.0))
        .border_1()
        .border_dashed()
        .border_color(c(0x403020FF))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_size(px(10.0))
                .font_family(SERIF)
                .text_color(c(0x5A4F40FF))
                .child("next arrival"),
        )
}

// All mode content rendered via builder pipeline.
// See holon_frontend::widget_gallery::{orient_mode_expr, flow_mode_expr,
// capture_mode_expr, chat_mode_expr}
