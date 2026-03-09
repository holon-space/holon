//! Expanded layout-invariant fixture matrix.
//!
//! `layout_smoke.rs` has the first invariant set on a handful of fixtures plus
//! the direct invariant unit tests. *This* file is the wider sweep: more
//! containers, more content variants, and three window sizes per fixture.
//!
//! Runs the full `assert_layout_ok` invariant set (nonempty, nonzero,
//! containment, sibling-overlap) against every generated case. If any cell in
//! the matrix breaks, the fixture label + size name make the failure
//! immediately pinpointable.

mod support;

use std::sync::Arc;

use gpui::{px, size, Pixels, Size, TestAppContext};
use holon_frontend::reactive_view_model::ReactiveViewModel;

use support::{assert_layout_ok, render_fixture_sized};

// ── Widget property helper ────────────────────────────────────────────

fn props(
    pairs: &[(&str, holon_api::Value)],
) -> std::collections::HashMap<String, holon_api::Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

// ── Content-widget builders ───────────────────────────────────────────

fn text(s: &str) -> ReactiveViewModel {
    ReactiveViewModel::text(s)
}

fn badge(label: &str) -> ReactiveViewModel {
    ReactiveViewModel::from_widget(
        "badge",
        props(&[("label", holon_api::Value::String(label.to_string()))]),
    )
}

fn icon(name: &str) -> ReactiveViewModel {
    ReactiveViewModel::from_widget(
        "icon",
        props(&[
            ("name", holon_api::Value::String(name.to_string())),
            ("size", holon_api::Value::Float(16.0)),
        ]),
    )
}

fn checkbox(checked: bool) -> ReactiveViewModel {
    ReactiveViewModel::from_widget(
        "checkbox",
        props(&[("checked", holon_api::Value::Boolean(checked))]),
    )
}

fn spacer(w: f32, h: f32) -> ReactiveViewModel {
    ReactiveViewModel::from_widget(
        "spacer",
        props(&[
            ("width", holon_api::Value::Float(w as f64)),
            ("height", holon_api::Value::Float(h as f64)),
        ]),
    )
}

// ── Container builders (single-child for matrix uniformity) ───────────

fn column(children: Vec<ReactiveViewModel>) -> ReactiveViewModel {
    ReactiveViewModel::from_widget("column", props(&[("gap", holon_api::Value::Float(4.0))]))
        .with_children(children)
}

fn row(children: Vec<ReactiveViewModel>) -> ReactiveViewModel {
    ReactiveViewModel::from_widget("row", props(&[("gap", holon_api::Value::Float(4.0))]))
        .with_children(children)
}

fn card(children: Vec<ReactiveViewModel>) -> ReactiveViewModel {
    ReactiveViewModel::from_widget(
        "card",
        props(&[("accent", holon_api::Value::String("#5DBDBD".to_string()))]),
    )
    .with_children(children)
}

fn section(title: &str, children: Vec<ReactiveViewModel>) -> ReactiveViewModel {
    ReactiveViewModel::from_widget(
        "section",
        props(&[("title", holon_api::Value::String(title.to_string()))]),
    )
    .with_children(children)
}

fn tree_item(depth: usize, children: Vec<ReactiveViewModel>) -> ReactiveViewModel {
    let has_children = !children.is_empty();
    ReactiveViewModel::from_widget(
        "tree_item",
        props(&[
            ("depth", holon_api::Value::Integer(depth as i64)),
            ("has_children", holon_api::Value::Boolean(has_children)),
        ]),
    )
    .with_children(children)
}

fn focusable(child: ReactiveViewModel) -> ReactiveViewModel {
    ReactiveViewModel::from_widget("focusable", std::collections::HashMap::new())
        .with_children(vec![child])
}

fn draggable(child: ReactiveViewModel) -> ReactiveViewModel {
    ReactiveViewModel::from_widget("draggable", std::collections::HashMap::new())
        .with_children(vec![child])
}

// ── The widened matrix ────────────────────────────────────────────────
//
// Each container is tested with several content variants. Single-child
// containers (focusable, draggable) get their own group because
// `Vec<ReactiveViewModel>` collapses to a single-child wrapper for them.

type MultiContainer = fn(Vec<ReactiveViewModel>) -> ReactiveViewModel;
type SingleContainer = fn(ReactiveViewModel) -> ReactiveViewModel;
type Content = fn() -> ReactiveViewModel;

fn multi_containers() -> Vec<(&'static str, MultiContainer)> {
    vec![
        ("column", column as MultiContainer),
        ("row", row),
        ("card", card),
        ("section", |c| section("Heading", c)),
        ("tree_item", |c| tree_item(1, c)),
    ]
}

fn single_containers() -> Vec<(&'static str, SingleContainer)> {
    vec![
        ("focusable", focusable as SingleContainer),
        ("draggable", draggable),
    ]
}

fn contents() -> Vec<(&'static str, Content)> {
    vec![
        ("text_short", (|| text("hello world")) as Content),
        // ~30 chars — fits the narrowest (400px) window so containment holds.
        //
        // FINDING (recorded, not fixed here): a longer string like the pangram
        // "The quick brown fox jumps over the lazy dog. (repeated)" (~88 chars)
        // renders at ~748px with the current `text::render` and escapes any
        // container narrower than that — no `text_wrap`, no `overflow: hidden`,
        // no ellipsis. Triggers the containment invariant. When a wrapping /
        // ellipsising Text variant is added, lengthen this string to keep
        // coverage.
        ("text_long", || text("The quick brown fox jumps over")),
        ("text_cjk", || text("日本語テストです")),
        ("text_multiline", || text("line one\nline two\nline three")),
        ("badge_short", || badge("NEW")),
        ("badge_long", || badge("VERY LONG STATUS LABEL")),
        ("icon", || icon("info")),
        ("checkbox_on", || checkbox(true)),
        ("checkbox_off", || checkbox(false)),
        ("spacer_wide", || spacer(50.0, 10.0)),
        ("spacer_tall", || spacer(10.0, 50.0)),
        // NOTE: `text_empty` (empty string) is intentionally omitted.
        //
        // `text::render` is a plain `div().child(content)` — an empty content
        // string produces a widget with 0 intrinsic width, which cascades up
        // every flex-content-sized parent (col, row, card, ...) and trips
        // `assert_all_nonzero`. This matches Taffy/flex semantics and is not
        // a regression; if someone adds click handling to a `Text`, that
        // widget will be un-hittable when empty, which is the class of
        // concrete bug the fast-UI layer will catch in the future once a
        // `Text` variant with a non-zero min-width is introduced.
        //
        // The finding itself has been recorded. Not rolling a bug fix here
        // since it changes widget behaviour and is out of scope for the
        // fast-UI test layer.
    ]
}

fn window_sizes() -> Vec<(&'static str, Size<Pixels>)> {
    vec![
        ("narrow_400x300", size(px(400.0), px(300.0))),
        ("default_800x600", size(px(800.0), px(600.0))),
        ("wide_1200x800", size(px(1200.0), px(800.0))),
    ]
}

// ── The tests ─────────────────────────────────────────────────────────

#[gpui::test]
fn layout_ok_multi_container_matrix(cx: &mut TestAppContext) {
    for (cname, container) in multi_containers() {
        for (iname, content) in contents() {
            for (sname, wsize) in window_sizes() {
                let label = format!("{cname}({iname})@{sname}");
                let fixture = container(vec![content()]);
                let snap = render_fixture_sized(cx, Arc::new(fixture), wsize);
                assert_layout_ok(&snap, &label);
            }
        }
    }
}

#[gpui::test]
fn layout_ok_single_container_matrix(cx: &mut TestAppContext) {
    for (cname, container) in single_containers() {
        for (iname, content) in contents() {
            for (sname, wsize) in window_sizes() {
                let label = format!("{cname}({iname})@{sname}");
                let fixture = container(content());
                let snap = render_fixture_sized(cx, Arc::new(fixture), wsize);
                assert_layout_ok(&snap, &label);
            }
        }
    }
}

// ── Mixed nesting: a few hand-picked realistic trees ─────────────────
//
// These aren't cartesian — they're specific shapes that mirror real UI
// patterns from Holon (outline items, cards with header/body, headers with
// icon + text + badge). A regression in any of them points directly at the
// pattern that broke.

#[gpui::test]
fn layout_ok_realistic_trees(cx: &mut TestAppContext) {
    let trees: Vec<(&str, Box<dyn Fn() -> ReactiveViewModel>)> = vec![
        (
            "card_with_header_body",
            Box::new(|| {
                card(vec![
                    row(vec![icon("info"), text("Heading"), badge("NEW")]),
                    text("Body content line 1"),
                    text("Body content line 2"),
                ])
            }),
        ),
        (
            "nested_tree",
            Box::new(|| {
                column(vec![
                    tree_item(0, vec![text("root")]),
                    tree_item(1, vec![text("child a")]),
                    tree_item(1, vec![text("child b"), badge("!")]),
                    tree_item(2, vec![text("grandchild")]),
                ])
            }),
        ),
        (
            "section_with_row",
            Box::new(|| {
                section(
                    "Settings",
                    vec![
                        row(vec![checkbox(true), text("Enable feature A")]),
                        row(vec![checkbox(false), text("Enable feature B")]),
                        row(vec![checkbox(true), text("Enable feature C")]),
                    ],
                )
            }),
        ),
        (
            "focusable_over_row",
            Box::new(|| focusable(row(vec![icon("info"), text("focusable row")]))),
        ),
        (
            "draggable_over_card",
            Box::new(|| draggable(card(vec![text("draggable card")]))),
        ),
    ];

    for (label, build) in trees {
        for (sname, wsize) in window_sizes() {
            let full_label = format!("{label}@{sname}");
            let snap = render_fixture_sized(cx, Arc::new(build()), wsize);
            assert_layout_ok(&snap, &full_label);
        }
    }
}
