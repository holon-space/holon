//! Layout smoke tests — first pass of the fast-UI test layer.
//!
//! Hand-written fixture matrix over pure-layout containers and content
//! widgets. Every fixture is rendered via `support::render_fixture` and
//! asserted against the "nonzero size" layout invariant — the regression
//! class that manifested most recently as the "Apr 6 cascade" bug.
//!
//! These tests:
//!   - Do NOT touch the backend (no DB, no reactive engine, no tokio runtime)
//!   - Do NOT open a real window (headless `TestPlatform`)
//!   - Run in milliseconds
//!   - Can run in parallel under `cargo nextest`
//!
//! When one fails, the fixture label + full `BoundsSnapshot` dump tell you
//! exactly which (container, content) combination collapsed.

mod support;

use std::sync::Arc;

use gpui::TestAppContext;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::reactive_view_model::{ReactiveViewKind, ReactiveViewModel};

use support::{
    assert_all_nonzero, assert_containment, assert_layout_ok, assert_no_sibling_overlap,
    assert_nonempty, render_fixture, BoundsSnapshot,
};

// ── Content-widget builders (leaf nodes) ───────────────────────────────

fn text(s: &str) -> ReactiveViewModel {
    ReactiveViewModel::text(s)
}

fn badge(label: &str) -> ReactiveViewModel {
    ReactiveViewModel::from_kind(ReactiveViewKind::Badge {
        label: label.to_string(),
    })
}

fn icon(name: &str) -> ReactiveViewModel {
    ReactiveViewModel::from_kind(ReactiveViewKind::Icon {
        name: name.to_string(),
        size: 16.0,
    })
}

// ── Container builders (single-child for simplicity) ──────────────────

fn col(children: Vec<ReactiveViewModel>) -> ReactiveViewModel {
    ReactiveViewModel::from_kind(ReactiveViewKind::Col {
        gap: 4.0,
        children: children.into_iter().map(Arc::new).collect(),
    })
}

fn row(children: Vec<ReactiveViewModel>) -> ReactiveViewModel {
    ReactiveViewModel::from_kind(ReactiveViewKind::Row {
        gap: 4.0,
        children: children.into_iter().map(Arc::new).collect(),
    })
}

fn card(children: Vec<ReactiveViewModel>) -> ReactiveViewModel {
    ReactiveViewModel::from_kind(ReactiveViewKind::Card {
        accent: "#5DBDBD".to_string(),
        children: children.into_iter().map(Arc::new).collect(),
    })
}

// ── The matrix ─────────────────────────────────────────────────────────

type Container = fn(Vec<ReactiveViewModel>) -> ReactiveViewModel;
type Content = fn() -> ReactiveViewModel;

/// Every (container, content) pair we smoke-test. Each pair is rendered as
/// `container(vec![content()])` and expected to produce nonzero bounds.
fn matrix() -> Vec<(&'static str, Container, &'static str, Content)> {
    vec![
        (
            "col",
            col as Container,
            "text",
            (|| text("hello")) as Content,
        ),
        ("col", col, "badge", || badge("NEW")),
        ("col", col, "icon", || icon("info")),
        ("row", row, "text", || text("hello")),
        ("row", row, "badge", || badge("NEW")),
        ("row", row, "icon", || icon("info")),
        ("card", card, "text", || text("hello")),
        ("card", card, "badge", || badge("NEW")),
        ("card", card, "icon", || icon("info")),
    ]
}

#[gpui::test]
fn layout_ok_matrix(cx: &mut TestAppContext) {
    for (cname, container, iname, content) in matrix() {
        let label = format!("{cname}({iname})");
        let fixture = container(vec![content()]);
        let snap = render_fixture(cx, Arc::new(fixture));

        // Full invariant set: nonempty, nonzero, containment, sibling-overlap.
        assert_layout_ok(&snap, &label);
    }
}

/// Real-render containment check on a fixture with multiple siblings. If any
/// flex child spills outside its `col` parent, containment fires.
#[gpui::test]
fn containment_col_of_text_and_badge(cx: &mut TestAppContext) {
    let fixture = col(vec![text("first"), text("second"), badge("LBL")]);
    let snap = render_fixture(cx, Arc::new(fixture));
    assert_layout_ok(&snap, "col(text, text, badge)");
}

// ── Nesting sweep: catch min-height propagation bugs past depth 2 ─────

#[gpui::test]
fn nonzero_bounds_nested_col_text(cx: &mut TestAppContext) {
    // col(col(col(text))) — was broken when `min_h_0()` was used instead of
    // `size_full()` in intermediate divs (Apr 6 cascade investigation).
    for depth in 1..=5 {
        let mut vm = text("depth-check");
        for _ in 0..depth {
            vm = col(vec![vm]);
        }
        let snap = render_fixture(cx, Arc::new(vm));
        let label = format!("col^{depth}(text)");
        assert_nonempty(&snap, &label);
        assert_all_nonzero(&snap, &label);
    }
}

// ── Negative test: prove the invariant actually fires ─────────────────
//
// A `Spacer { width: 0, height: 0 }` inside a `Row` produces a zero-sized
// tracker entry. If `assert_all_nonzero` silently accepted this, every
// positive test above would be vacuous. `#[should_panic]` proves the
// invariant is load-bearing.

#[gpui::test]
#[should_panic(expected = "layout invariant violated (nonzero)")]
fn invariant_fires_on_zero_spacer(cx: &mut TestAppContext) {
    let fixture = row(vec![ReactiveViewModel::from_kind(
        ReactiveViewKind::Spacer {
            width: 0.0,
            height: 0.0,
            color: None,
        },
    )]);
    let snap = render_fixture(cx, Arc::new(fixture));
    assert_all_nonzero(&snap, "row(spacer(0,0))");
}

// ── Sanity: the snapshot actually captures every widget ───────────────

#[gpui::test]
fn snapshot_captures_each_widget(cx: &mut TestAppContext) {
    // col(text, text, badge) — the snapshot should contain one entry for
    // `col` and one for each of the four leaves, proving `tag()` is wired
    // through `builder_registry!` for every variant.
    let fixture = col(vec![text("a"), text("b"), badge("c"), icon("info")]);
    let snap = render_fixture(cx, Arc::new(fixture));

    let col_count = snap.of_type("col").count();
    let text_count = snap.of_type("text").count();
    let badge_count = snap.of_type("badge").count();
    let icon_count = snap.of_type("icon").count();

    assert_eq!(
        col_count,
        1,
        "expected 1 col, got {col_count}\n{}",
        snap.dump()
    );
    assert_eq!(
        text_count,
        2,
        "expected 2 text, got {text_count}\n{}",
        snap.dump()
    );
    assert_eq!(
        badge_count,
        1,
        "expected 1 badge, got {badge_count}\n{}",
        snap.dump()
    );
    assert_eq!(
        icon_count,
        1,
        "expected 1 icon, got {icon_count}\n{}",
        snap.dump()
    );
}

// ── Direct invariant unit tests ───────────────────────────────────────
//
// These craft a `BoundsSnapshot` by hand so they can deliberately construct
// violations (something real builders won't produce). Together with the real
// matrix tests above they prove that the invariant code is load-bearing —
// not just accidentally passing because real layouts never get close to the
// thresholds.

/// Helper to build a fake snapshot entry. Keeps the tests below terse.
fn fake(
    id: &str,
    widget: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    parent: Option<&str>,
) -> (String, ElementInfo) {
    (
        id.to_string(),
        ElementInfo {
            x,
            y,
            width: w,
            height: h,
            widget_type: widget.to_string(),
            entity_id: None,
            has_content: false,
            parent_id: parent.map(|s| s.to_string()),
        },
    )
}

fn fake_snapshot(entries: Vec<(String, ElementInfo)>) -> BoundsSnapshot {
    BoundsSnapshot { entries }
}

#[test]
fn containment_passes_on_valid_tree() {
    let snap = fake_snapshot(vec![
        fake("root#0", "col", 0.0, 0.0, 100.0, 200.0, None),
        fake("text#1", "text", 0.0, 0.0, 50.0, 20.0, Some("root#0")),
        fake("text#2", "text", 0.0, 24.0, 60.0, 20.0, Some("root#0")),
    ]);
    assert_containment(&snap, "valid", &[]);
    assert_no_sibling_overlap(&snap, "valid", &[]);
}

#[test]
#[should_panic(expected = "layout invariant violated (containment)")]
fn containment_fires_when_child_escapes_parent() {
    let snap = fake_snapshot(vec![
        fake("root#0", "col", 0.0, 0.0, 100.0, 100.0, None),
        // Child starts at x=80 with width=50 → right edge at 130, escapes 100.
        fake("text#1", "text", 80.0, 0.0, 50.0, 20.0, Some("root#0")),
    ]);
    assert_containment(&snap, "escaping-child", &[]);
}

#[test]
fn containment_allows_listed_overflow_widgets() {
    // An off-parent `drawer` is legitimate (drawer renders floating). The
    // allow-list should make containment ignore it.
    let snap = fake_snapshot(vec![
        fake("root#0", "col", 0.0, 0.0, 100.0, 100.0, None),
        fake(
            "drawer#1",
            "drawer",
            200.0,
            200.0,
            50.0,
            50.0,
            Some("root#0"),
        ),
    ]);
    assert_containment(&snap, "drawer-overflow", &["drawer"]);
}

#[test]
#[should_panic(expected = "layout invariant violated (sibling overlap)")]
fn sibling_overlap_fires_on_overlapping_children() {
    let snap = fake_snapshot(vec![
        fake("root#0", "col", 0.0, 0.0, 100.0, 100.0, None),
        fake("a#1", "text", 0.0, 0.0, 60.0, 40.0, Some("root#0")),
        // Overlaps a#1: 20..60 × 20..40.
        fake("b#2", "text", 20.0, 20.0, 60.0, 40.0, Some("root#0")),
    ]);
    assert_no_sibling_overlap(&snap, "overlapping-siblings", &[]);
}

#[test]
fn sibling_overlap_tolerates_shared_edges() {
    // Two flex children stacked vertically with y1 of the first == y0 of the
    // second. Real flex layouts produce this constantly; the epsilon must let
    // it pass.
    let snap = fake_snapshot(vec![
        fake("root#0", "col", 0.0, 0.0, 100.0, 100.0, None),
        fake("a#1", "text", 0.0, 0.0, 100.0, 50.0, Some("root#0")),
        fake("b#2", "text", 0.0, 50.0, 100.0, 50.0, Some("root#0")),
    ]);
    assert_no_sibling_overlap(&snap, "shared-edge", &[]);
}

#[test]
fn sibling_overlap_ignores_unrelated_subtrees() {
    // Siblings under DIFFERENT parents must never be compared. Otherwise a
    // valid layout where one card is next to another card but their children
    // happen to have the same local coordinates would trigger false overlap.
    let snap = fake_snapshot(vec![
        fake("root#0", "col", 0.0, 0.0, 200.0, 100.0, None),
        fake("card_a#1", "card", 0.0, 0.0, 100.0, 100.0, Some("root#0")),
        fake("text_a#2", "text", 0.0, 0.0, 50.0, 20.0, Some("card_a#1")),
        fake("card_b#3", "card", 100.0, 0.0, 100.0, 100.0, Some("root#0")),
        fake("text_b#4", "text", 100.0, 0.0, 50.0, 20.0, Some("card_b#3")),
    ]);
    // text_a and text_b have the same global rect shape but live under
    // different parents — no sibling overlap violation.
    assert_no_sibling_overlap(&snap, "two-cards", &[]);
    assert_containment(&snap, "two-cards", &[]);
}
