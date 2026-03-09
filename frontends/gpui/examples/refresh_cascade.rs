//! Test different uniform_list wrapping strategies.
//!
//! Each panel tests a different nesting pattern to find what breaks uniform_list.
//! Run with: cargo run --example refresh_cascade -p holon-gpui

use gpui::prelude::*;
use gpui::*;

// --- ItemView (simple leaf) ---

struct ItemView {
    label: String,
}

impl Render for ItemView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .text_sm()
            .border_b_1()
            .border_color(rgb(0x333333))
            .child(self.label.clone())
    }
}

// --- ListEntity (wraps uniform_list in a View entity, like CollectionView) ---

struct ListEntity {
    items: Vec<Entity<ItemView>>,
}

impl Render for ListEntity {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.items.clone();
        uniform_list("list-entity", items.len(), move |range, _window, _cx| {
            range.map(|i| items[i].clone().into_any_element()).collect()
        })
        .flex_1()
        .size_full()
    }
}

// --- AppView ---

struct AppView {
    items: Vec<Entity<ItemView>>,
    list_entity: Entity<ListEntity>,
}

impl AppView {
    fn new(cx: &mut Context<Self>) -> Self {
        let items: Vec<_> = (0..100)
            .map(|i| {
                cx.new(|_cx| ItemView {
                    label: format!("Item {i}"),
                })
            })
            .collect();
        let list_entity = cx.new(|_cx| ListEntity {
            items: items.clone(),
        });
        Self { items, list_entity }
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.items.clone();
        let items2 = self.items.clone();
        let items3 = self.items.clone();
        let items4 = self.items.clone();
        let items5 = self.items.clone();

        div()
            .flex()
            .size_full()
            .bg(rgb(0x1e1e1e))
            .text_color(rgb(0xcccccc))
            .gap_2()
            .p_2()
            .children([
                // Panel A: uniform_list as direct child of root flex_col (KNOWN WORKING)
                panel(
                    "A: direct child",
                    div().flex_1().size_full().flex_col().child(
                        uniform_list("list-a", items.len(), move |range, _w, _cx| {
                            range.map(|i| items[i].clone().into_any_element()).collect()
                        })
                        .flex_1()
                        .size_full(),
                    ),
                ),
                // Panel B: uniform_list inside ONE intermediate div
                panel(
                    "B: 1 wrapper div",
                    div().flex_1().size_full().flex_col().child(
                        div().flex_1().flex_col().child(
                            uniform_list("list-b", items2.len(), move |range, _w, _cx| {
                                range
                                    .map(|i| items2[i].clone().into_any_element())
                                    .collect()
                            })
                            .flex_1()
                            .size_full(),
                        ),
                    ),
                ),
                // Panel C: uniform_list inside a View entity (like CollectionView)
                panel(
                    "C: View entity",
                    div()
                        .flex_1()
                        .size_full()
                        .flex_col()
                        .child(self.list_entity.clone()),
                ),
                // Panel D: uniform_list inside section-like div (w_full, flex_col, NO flex_1)
                panel(
                    "D: section (no flex_1)",
                    div().flex_1().size_full().flex_col().child(
                        div().w_full().flex_col().child(
                            uniform_list("list-d", items3.len(), move |range, _w, _cx| {
                                range
                                    .map(|i| items3[i].clone().into_any_element())
                                    .collect()
                            })
                            .flex_1()
                            .size_full(),
                        ),
                    ),
                ),
                // Panel E: uniform_list inside section div WITH flex_1 and size_full
                panel(
                    "E: section+flex_1+size_full",
                    div().flex_1().size_full().flex_col().child(
                        div().flex_1().size_full().flex_col().child(
                            uniform_list("list-e", items4.len(), move |range, _w, _cx| {
                                range
                                    .map(|i| items4[i].clone().into_any_element())
                                    .collect()
                            })
                            .flex_1()
                            .size_full(),
                        ),
                    ),
                ),
                // Panel F: View entity wrapped in .cached()
                panel(
                    "F: cached View entity",
                    div().flex_1().size_full().flex_col().child({
                        let mut s = StyleRefinement::default();
                        s.flex_grow = Some(1.0);
                        s.size.width = Some(relative(1.0).into());
                        s.size.height = Some(relative(1.0).into());
                        AnyView::from(self.list_entity.clone()).cached(s)
                    }),
                ),
                // Panel G: View entity inside section with size_full (WORKS)
                panel(
                    "G: View in section+size_full",
                    div().flex_1().size_full().flex_col().child(
                        div()
                            .size_full()
                            .flex_1()
                            .flex_col()
                            .child(div().child("Header"))
                            .child(self.list_entity.clone()),
                    ),
                ),
                // Panel H: columns.rs pattern — absolute+scroll parent around View entity
                panel(
                    "H: abs+scroll+View",
                    div().flex_1().relative().child(
                        div()
                            .id("scroll-h")
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .overflow_y_scroll()
                            .child(self.list_entity.clone()),
                    ),
                ),
                // Panel I: Holon chain (broken — diagnosing)
                panel_holon_chain(self.list_entity.clone()),
                // Panel J: G + overflow_hidden on outer
                panel(
                    "J: G+overflow_hidden",
                    div()
                        .flex_1()
                        .size_full()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .size_full()
                                .flex_1()
                                .flex_col()
                                .child(div().child("Header"))
                                .child(self.list_entity.clone()),
                        ),
                ),
                // Panel K: G + overflow_hidden + padding
                panel(
                    "K: G+ovfl+padding",
                    div()
                        .flex_1()
                        .size_full()
                        .flex_col()
                        .overflow_hidden()
                        .px(px(32.0))
                        .py(px(12.0))
                        .child(
                            div()
                                .size_full()
                                .flex_1()
                                .flex_col()
                                .child(div().child("Header"))
                                .child(self.list_entity.clone()),
                        ),
                ),
                // Panel L: G with min_h_0 instead of size_full on outer
                panel(
                    "L: G-min_h_0",
                    div().flex_1().min_h_0().flex_col().child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex_col()
                            .child(div().child("Header"))
                            .child(self.list_entity.clone()),
                    ),
                ),
                // Panel M: G with cached() wrapper on entity
                panel(
                    "M: G+cached",
                    div().flex_1().size_full().flex_col().child(
                        div()
                            .size_full()
                            .flex_1()
                            .flex_col()
                            .child(div().child("Header"))
                            .child({
                                let mut s = StyleRefinement::default();
                                s.flex_grow = Some(1.0);
                                s.size.width = Some(relative(1.0).into());
                                s.min_size.height = Some(px(0.0).into());
                                AnyView::from(self.list_entity.clone()).cached(s)
                            }),
                    ),
                ),
            ])
    }
}

fn panel(title: &str, content: impl IntoElement) -> AnyElement {
    div()
        .flex_1()
        .size_full()
        .flex_col()
        .border_1()
        .border_color(rgb(0x444444))
        .rounded_md()
        .overflow_hidden()
        .child(
            div()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(rgb(0x444444))
                .text_xs()
                .child(title.to_string()),
        )
        .child(content)
        .into_any_element()
}

/// Reproduce the exact Holon GPUI hierarchy:
/// columns row → panel div → view_mode_switcher div → CollectionView entity
fn panel_holon_chain(list_entity: Entity<ListEntity>) -> AnyElement {
    // Outer: panel() wrapper from the example (flex_1 + size_full + flex_col + border)
    div()
        .flex_1()
        .size_full()
        .flex_col()
        .border_1()
        .border_color(rgb(0x444444))
        .rounded_md()
        .overflow_hidden()
        .child(
            div()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(rgb(0x444444))
                .text_xs()
                .child("I: Holon chain"),
        )
        // This is what columns.rs produces for the main panel:
        .child(
            div()
                .id("panel-i")
                .flex_1()
                .size_full()
                .overflow_hidden()
                .flex_col()
                .px(px(32.0))
                .py(px(12.0))
                // This is what view_mode_switcher produces:
                .child(
                    div()
                        .size_full()
                        .flex_1()
                        .flex_col()
                        // Switcher bar (header element)
                        .child(div().flex().justify_end().child("icons"))
                        // CollectionView entity with cached style
                        .child({
                            let mut s = StyleRefinement::default();
                            s.flex_grow = Some(1.0);
                            s.size.width = Some(relative(1.0).into());
                            s.size.height = Some(relative(1.0).into());
                            AnyView::from(list_entity).cached(s)
                        }),
                ),
        )
        .into_any_element()
}

fn main() {
    let app = Application::with_platform(gpui_platform::current_platform(false));
    app.run(move |cx: &mut App| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(50.0), px(50.0)),
                    size: size(px(1400.0), px(600.0)),
                })),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| AppView::new(cx)),
        )
        .expect("Failed to open window");
    });
}
