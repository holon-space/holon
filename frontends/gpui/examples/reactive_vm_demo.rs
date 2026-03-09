//! Reactive ViewModel demo — interactive proof-of-concept.
//!
//! Run with: cargo run --example reactive_vm_demo -p holon-gpui

use std::sync::{Arc, Mutex};

use futures_signals::signal::{Mutable, SignalExt};
use gpui::prelude::*;
use gpui::*;

use holon_api::render_types::RenderExpr;
use holon_api::Value;

use holon_gpui::reactive_vm_poc::*;

// ── Panel A: ExpandToggle ──────────────────────────────────────────────

struct PanelA {
    expanded: Mutable<bool>,
    header_text: String,
    content_items: Vec<String>,
    generation: usize,
}

impl PanelA {
    fn new(cx: &mut Context<Self>) -> Self {
        let expanded = Mutable::new(false);
        let data = make_sample_data();
        let template = tree_template();
        let content_items: Vec<String> = data
            .iter()
            .map(|row| interpret_expr(&template, row))
            .collect();

        let signal = expanded.signal();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            let mut stream = signal.to_stream();
            stream.next().await;
            while stream.next().await.is_some() {
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();

        Self {
            expanded,
            header_text: "My Tasks (click ▶ to expand)".into(),
            content_items,
            generation: 0,
        }
    }

    fn simulate_data_change(&mut self, _cx: &mut Context<Self>) {
        self.generation += 1;
        let mut data = make_sample_data();
        if let Some(row) = data.first_mut() {
            row.insert(
                "content".into(),
                Value::String(format!("Buy milk (gen {})", self.generation)),
            );
        }
        let template = tree_template();
        self.content_items = data
            .iter()
            .map(|row| interpret_expr(&template, row))
            .collect();
        self.header_text = format!("My Tasks (gen {})", self.generation);
    }
}

impl Render for PanelA {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_expanded = self.expanded.get();
        let chevron = if is_expanded { "▼" } else { "▶" };

        let toggle_handle = self.expanded.clone();
        let chevron_el = div()
            .id("expand-chevron")
            .cursor_pointer()
            .px_2()
            .text_sm()
            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                toggle_handle.set(!toggle_handle.get());
            })
            .child(chevron.to_string());

        let header_row = div()
            .flex()
            .gap_2()
            .py_1()
            .child(chevron_el)
            .child(div().text_sm().child(self.header_text.clone()));

        let btn = btn_el(
            "a-btn",
            "🔄 Data change",
            rgb(0x333355),
            cx,
            |this: &mut Self, cx| {
                this.simulate_data_change(cx);
                cx.notify();
            },
        );

        let mut col = div().w_full().flex_col().gap_1().child(header_row);
        if is_expanded {
            for item in &self.content_items {
                col = col.child(
                    div()
                        .pl_4()
                        .py(px(1.0))
                        .text_sm()
                        .font_family("monospace")
                        .child(item.clone()),
                );
            }
        }
        col.child(btn).child(
            div()
                .mt_2()
                .text_xs()
                .text_color(rgb(0x888888))
                .child(format!("expanded={}  gen={}", is_expanded, self.generation)),
        )
    }
}

// ── Panel B: Push-down self-interpreting collection ────────────────────

struct PanelB {
    active_mode: Mutable<String>,
    modes: Vec<(String, RenderExpr)>,
    item_template: Mutable<RenderExpr>,
    items: Vec<Entity<ItemNode>>,
    raw_data: Vec<Arc<DataRow>>,
    event_log: Arc<Mutex<Vec<String>>>,
}

impl PanelB {
    fn new(cx: &mut Context<Self>) -> Self {
        let modes = vec![
            ("tree".into(), tree_template()),
            ("table".into(), table_template()),
            ("compact".into(), compact_template()),
        ];
        let active_mode = Mutable::new("tree".to_string());
        let item_template = Mutable::new(tree_template());
        let raw_data: Vec<Arc<DataRow>> = make_sample_data().into_iter().map(Arc::new).collect();

        let items: Vec<Entity<ItemNode>> = raw_data
            .iter()
            .map(|row| {
                let tmpl = item_template.clone();
                let row = row.clone();
                cx.new(|cx| ItemNode::new(row, tmpl, default_interpreter(), cx))
            })
            .collect();

        let event_log = Arc::new(Mutex::new(Vec::<String>::new()));

        {
            let mode_signal = active_mode.signal_cloned();
            let modes_clone = modes.clone();
            let tmpl_handle = item_template.clone();
            let log = event_log.clone();
            cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                let mut stream = mode_signal.to_stream();
                stream.next().await;
                while let Some(mode) = stream.next().await {
                    if let Some((_, e)) = modes_clone.iter().find(|(n, _)| n == &mode) {
                        tmpl_handle.set(e.clone());
                        log.lock().unwrap().push(format!("mode→{mode}"));
                        let _ = this.update(cx, |_, cx| cx.notify());
                    }
                }
            })
            .detach();
        }

        Self {
            active_mode,
            modes,
            item_template,
            items,
            raw_data,
            event_log,
        }
    }

    fn simulate_cdc_update(&mut self, _cx: &mut Context<Self>) {
        if let Some(first) = self.raw_data.first().cloned() {
            let mut updated = (*first).clone();
            let old = updated
                .get("content")
                .map(|v| v.to_display_string())
                .unwrap_or_default();
            updated.insert("content".into(), Value::String(format!("{old}!")));
            let new_data = Arc::new(updated);
            self.raw_data[0] = new_data.clone();
            self.items[0].read(_cx).data.set(new_data);
            self.event_log.lock().unwrap().push("cdc_update[0]".into());
        }
    }

    fn simulate_cdc_insert(&mut self, cx: &mut Context<Self>) {
        let id = self.raw_data.len() + 1;
        let data = Arc::new(make_row(&format!("{id}"), &format!("New #{id}"), "new"));
        self.raw_data.push(data.clone());
        let tmpl = self.item_template.clone();
        self.items
            .push(cx.new(|cx| ItemNode::new(data, tmpl, default_interpreter(), cx)));
        self.event_log.lock().unwrap().push("cdc_insert".into());
        cx.notify();
    }

    fn simulate_cdc_delete(&mut self, cx: &mut Context<Self>) {
        if self.items.len() > 1 {
            self.raw_data.pop();
            self.items.pop();
            self.event_log.lock().unwrap().push("cdc_delete".into());
            cx.notify();
        }
    }
}

impl Render for PanelB {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_mode = self.active_mode.get_cloned();
        let mode_buttons = div()
            .flex()
            .gap_2()
            .children(self.modes.iter().map(|(mode, _)| {
                let is_active = *mode == current_mode;
                let h = self.active_mode.clone();
                let m = mode.clone();
                div()
                    .id(SharedString::from(format!("mode-{mode}")))
                    .cursor_pointer()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .when(is_active, |d| d.bg(rgb(0x4466aa)))
                    .when(!is_active, |d| d.bg(rgb(0x333333)))
                    .child(mode.clone())
                    .on_mouse_down(MouseButton::Left, move |_, _, _| h.set(m.clone()))
            }));

        let cdc_btns = div()
            .flex()
            .gap_2()
            .mt_2()
            .child(btn_el(
                "b-upd",
                "📝 Update[0]",
                rgb(0x335533),
                cx,
                |this: &mut Self, cx| this.simulate_cdc_update(cx),
            ))
            .child(btn_el(
                "b-ins",
                "➕ Insert",
                rgb(0x335533),
                cx,
                |this: &mut Self, cx| this.simulate_cdc_insert(cx),
            ))
            .child(btn_el(
                "b-del",
                "🗑 Delete",
                rgb(0x553333),
                cx,
                |this: &mut Self, cx| this.simulate_cdc_delete(cx),
            ));

        let log = self.event_log.lock().unwrap();
        let last: String = log
            .iter()
            .rev()
            .take(3)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join(" → ");

        div()
            .w_full()
            .flex_col()
            .gap_1()
            .child(mode_buttons)
            .child(
                div()
                    .flex_col()
                    .py_1()
                    .children(self.items.iter().map(|i| i.clone().into_any_element())),
            )
            .child(cdc_btns)
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x888888))
                    .child(format!("items={}  {last}", self.items.len())),
            )
    }
}

// ── Panel C: Structural Changes ────────────────────────────────────────

struct PanelC {
    root: Entity<ReactiveNode>,
    data: Arc<DataRow>,
    scenario_idx: usize,
    scenarios: Vec<(&'static str, RenderExpr)>,
}

impl PanelC {
    fn new(cx: &mut Context<Self>) -> Self {
        let data = Arc::new(make_row("demo", "Hello World", "high"));

        let scenarios: Vec<(&str, RenderExpr)> = vec![
            (
                "row(text, badge)",
                expr(
                    "row",
                    vec![
                        positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                        positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
                    ],
                ),
            ),
            (
                "row(text, badge, icon) — child added",
                expr(
                    "row",
                    vec![
                        positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                        positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
                        positional_arg(expr("icon", vec![positional_arg(literal_str("star"))])),
                    ],
                ),
            ),
            (
                "row(text) — children removed",
                expr(
                    "row",
                    vec![positional_arg(expr(
                        "text",
                        vec![positional_arg(col_ref("content"))],
                    ))],
                ),
            ),
            (
                "row(badge, text) — child types swapped",
                expr(
                    "row",
                    vec![
                        positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
                        positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                    ],
                ),
            ),
            (
                "row(bold(text), badge, spacer) — nested + new types",
                expr(
                    "row",
                    vec![
                        positional_arg(expr(
                            "bold",
                            vec![positional_arg(expr(
                                "text",
                                vec![positional_arg(col_ref("content"))],
                            ))],
                        )),
                        positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
                        positional_arg(expr("spacer", vec![])),
                    ],
                ),
            ),
        ];

        let root = cx.new(|cx| {
            ReactiveNode::new(
                scenarios[0].1.clone(),
                data.clone(),
                default_interpreter(),
                cx,
            )
        });

        Self {
            root,
            data,
            scenario_idx: 0,
            scenarios,
        }
    }

    fn next_scenario(&mut self, cx: &mut Context<Self>) {
        self.scenario_idx = (self.scenario_idx + 1) % self.scenarios.len();
        let new_expr = self.scenarios[self.scenario_idx].1.clone();
        self.root
            .update(cx, |root, cx| root.apply_expr(new_expr, cx));
        cx.notify();
    }

    fn change_data(&mut self, cx: &mut Context<Self>) {
        let old = self
            .data
            .get("content")
            .map(|v| v.to_display_string())
            .unwrap_or_default();
        let mut new_data = (*self.data).clone();
        new_data.insert("content".into(), Value::String(format!("{old}!")));
        self.data = Arc::new(new_data);
        self.root.update(cx, |root, cx| {
            root.data.set(self.data.clone());
            cx.notify();
        });
    }
}

impl Render for PanelC {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (label, _) = &self.scenarios[self.scenario_idx];

        div()
            .w_full()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(format!("Expr: {label}")),
            )
            .child(
                div()
                    .border_1()
                    .border_color(rgb(0x333355))
                    .rounded_md()
                    .p_2()
                    .child(self.root.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(btn_el(
                        "c-next",
                        "⏭ Next expr",
                        rgb(0x333355),
                        cx,
                        |this: &mut Self, cx| this.next_scenario(cx),
                    ))
                    .child(btn_el(
                        "c-data",
                        "📝 Change data",
                        rgb(0x335533),
                        cx,
                        |this: &mut Self, cx| {
                            this.change_data(cx);
                            cx.notify();
                        },
                    )),
            )
            .child(div().text_xs().text_color(rgb(0x888888)).child(format!(
                "scenario {}/{}",
                self.scenario_idx + 1,
                self.scenarios.len()
            )))
    }
}

// ── Shared button helper ───────────────────────────────────────────────

fn btn_el<V: 'static>(
    id: &str,
    label: &str,
    bg_color: Rgba,
    cx: &mut Context<V>,
    handler: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .cursor_pointer()
        .px_2()
        .py_1()
        .bg(bg_color)
        .rounded_md()
        .text_xs()
        .child(label.to_string())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| handler(this, cx)),
        )
}

// ── Root App ───────────────────────────────────────────────────────────

struct DemoApp {
    panel_a: Entity<PanelA>,
    panel_b: Entity<PanelB>,
    panel_c: Entity<PanelC>,
}

impl DemoApp {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            panel_a: cx.new(|cx| PanelA::new(cx)),
            panel_b: cx.new(|cx| PanelB::new(cx)),
            panel_c: cx.new(|cx| PanelC::new(cx)),
        }
    }
}

impl Render for DemoApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .bg(rgb(0x1a1a2e))
            .text_color(rgb(0xdddddd))
            .gap_4()
            .p_4()
            .child(panel_frame("A: ExpandToggle", self.panel_a.clone()))
            .child(panel_frame("B: Push-down Collection", self.panel_b.clone()))
            .child(panel_frame("C: Structural Changes", self.panel_c.clone()))
    }
}

fn panel_frame(title: &str, content: impl IntoElement) -> impl IntoElement {
    div()
        .flex_1()
        .size_full()
        .flex_col()
        .border_1()
        .border_color(rgb(0x444466))
        .rounded_lg()
        .overflow_hidden()
        .child(
            div()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(0x444466))
                .bg(rgb(0x16213e))
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .child(title.to_string()),
        )
        .child(div().flex_1().p_3().child(content))
}

fn main() {
    let app = Application::with_platform(gpui_platform::current_platform(false));
    app.run(move |cx: &mut App| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(100.0), px(100.0)),
                    size: size(px(1400.0), px(550.0)),
                })),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| DemoApp::new(cx)),
        )
        .expect("Failed to open window");
    });
}
