//! A tree row's leading chrome — disclosure chevron, leaf bullet, and the
//! sidebar page icon — belongs on the FIRST TEXT LINE, not on the row's
//! geometric center (Martin 2026-07-30, task #37).
//!
//! The row is top-aligned and grows downward as its content wraps, so the only
//! stable anchor a marker may center on is the first line box: its center sits
//! at `row.y + text_line_height / 2` whether the row is one line or five. A
//! marker centered on the whole row drifts further down with every added line,
//! and a marker with a hand-tuned `mt` lands near — but not on — the line even
//! at one line.
//!
//! Run: `cargo nextest run -p holon-gpui --test tree_row_first_line_alignment`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_frontend::expand_toggle_id_for;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::tree_bullet_id_for;
use holon_gpui::render::builders::style::LayoutStyle;
use support::BoundsSnapshot;
use support::render_fixture_sized;

/// Narrow enough that the row text below wraps onto a second line — a sidebar's
/// real width.
const VIEWPORT_W: f32 = 240.0;
const VIEWPORT_H: f32 = 400.0;

/// Long enough to wrap at `VIEWPORT_W`.
const WRAPPING_TEXT: &str = "A page title long enough to wrap onto a second line in the sidebar";

/// Tolerance in px. The assertion is about which LINE a glyph sits on, so a
/// sub-pixel rounding difference is not a defect — a 3px+ drift is.
const EPS: f32 = 1.5;

fn line_height() -> f32 {
    LayoutStyle::default().text_line_height
}

/// The row's leading chrome, as the row model prescribes it.
#[derive(Clone, Copy)]
enum Marker {
    Chevron,
    Bullet,
    /// The left sidebar's `row(icon("notebook"), spacer, text(...))` content.
    PageIcon,
}

fn text_leaf(content: &str) -> ReactiveViewModel {
    let mut props = HashMap::new();
    props.insert("content".to_string(), Value::String(content.to_string()));
    ReactiveViewModel::from_widget("text", props)
}

/// A production-shaped tree row: identity lives on the content child's entity
/// data, exactly as the live org tree builds it (see
/// `sidebar_disclosure_affordance.rs`).
fn tree_row(id: &str, marker: Marker) -> ReactiveViewModel {
    let mut props = HashMap::new();
    props.insert("depth".to_string(), Value::Float(0.0));
    props.insert(
        "has_children".to_string(),
        Value::Boolean(matches!(marker, Marker::Chevron)),
    );
    props.insert(
        "show_bullet".to_string(),
        Value::Boolean(matches!(marker, Marker::Bullet)),
    );

    let mut content =
        match marker {
            // `selectable(row(icon("notebook"), spacer(6), text(col("content"))))`
            // — the left sidebar's item template verbatim (assets/default/index.org,
            // `left_sidebar::render::0`).
            Marker::PageIcon => {
                let mut icon_props = HashMap::new();
                icon_props.insert("name".to_string(), Value::String("notebook".to_string()));
                let icon = ReactiveViewModel::from_widget("icon", icon_props);
                let mut spacer_props = HashMap::new();
                spacer_props.insert("width".to_string(), Value::Float(6.0));
                let spacer = ReactiveViewModel::from_widget("spacer", spacer_props);
                let row = ReactiveViewModel::from_widget("row", HashMap::new())
                    .with_children(vec![icon, spacer, text_leaf(WRAPPING_TEXT)]);
                ReactiveViewModel::from_widget("selectable", HashMap::new())
                    .with_children(vec![row])
            }
            _ => text_leaf(WRAPPING_TEXT),
        };
    let mut data = HashMap::new();
    data.insert("id".to_string(), Value::String(format!("block:{id}")));
    data.insert("content".to_string(), Value::String(WRAPPING_TEXT.into()));
    content.data = Mutable::new(Arc::new(data)).read_only();

    let mut vm = ReactiveViewModel::from_widget("tree_item", props).with_children(vec![content]);
    vm.expanded = Some(Mutable::new(true));
    vm.hovered = Some(Mutable::new(false));
    vm
}

fn column(rows: Vec<ReactiveViewModel>) -> Arc<ReactiveViewModel> {
    let mut col = ReactiveViewModel::from_widget("column", HashMap::new());
    col.children = rows.into_iter().map(Arc::new).collect();
    Arc::new(col)
}

fn by_id<'a>(snap: &'a BoundsSnapshot, el_id: &str) -> Option<&'a ElementInfo> {
    snap.entries
        .iter()
        .find(|(id, _)| id == el_id)
        .map(|(_, i)| i)
}

fn of_type<'a>(snap: &'a BoundsSnapshot, widget_type: &str) -> Vec<&'a ElementInfo> {
    let mut v: Vec<&ElementInfo> = snap
        .entries
        .iter()
        .map(|(_, i)| i)
        .filter(|i| &*i.widget_type == widget_type)
        .collect();
    v.sort_by(|a, b| a.y.total_cmp(&b.y));
    v
}

/// The `tree_item` box whose extent contains `marker`'s center — the row the
/// marker belongs to, without depending on element-registration order.
fn owning_row<'a>(snap: &'a BoundsSnapshot, marker: &ElementInfo) -> &'a ElementInfo {
    let (mx, my) = marker.center();
    of_type(snap, "tree_item")
        .into_iter()
        .find(|r| mx >= r.x && mx <= r.x + r.width && my >= r.y && my <= r.y + r.height)
        .unwrap_or_else(|| {
            panic!(
                "no tree_item box contains the marker at ({mx}, {my}):\n{}",
                snap.dump()
            )
        })
}

/// THE invariant: the marker's vertical center is the first line box's center.
fn assert_on_first_line(snap: &BoundsSnapshot, marker: &ElementInfo, what: &str) {
    let row = owning_row(snap, marker);
    assert!(
        row.height > line_height() + EPS,
        "{what}: the fixture must produce a WRAPPED (multi-line) row for this assertion to \
         discriminate; row height {} is a single line",
        row.height
    );
    let expected = row.y + line_height() / 2.0;
    let actual = marker.center().1;
    assert!(
        (actual - expected).abs() <= EPS,
        "{what}: marker center-y {actual:.2} must sit on the FIRST text line (center \
         {expected:.2} = row top {:.2} + line-height {:.2}/2); off by {:.2}px. Row is {:.2}px \
         tall, so a whole-row centering would land at {:.2}.",
        row.y,
        line_height(),
        actual - expected,
        row.height,
        row.y + row.height / 2.0
    );
}

fn render(cx: &mut TestAppContext, marker: Marker, id: &str) -> BoundsSnapshot {
    render_fixture_sized(
        cx,
        column(vec![tree_row(id, marker)]),
        size(px(VIEWPORT_W), px(VIEWPORT_H)),
    )
}

#[gpui::test]
fn chevron_centers_on_the_first_line_of_a_wrapped_row(cx: &mut TestAppContext) {
    let snap = render(cx, Marker::Chevron, "parent");
    let chevron = by_id(&snap, &expand_toggle_id_for("parent"))
        .unwrap_or_else(|| panic!("no chevron registered for the parent row:\n{}", snap.dump()));
    assert_on_first_line(&snap, chevron, "disclosure chevron");
}

#[gpui::test]
fn bullet_centers_on_the_first_line_of_a_wrapped_row(cx: &mut TestAppContext) {
    let snap = render(cx, Marker::Bullet, "leaf");
    let bullet = by_id(&snap, &tree_bullet_id_for("leaf"))
        .unwrap_or_else(|| panic!("no bullet registered for the leaf row:\n{}", snap.dump()));
    assert_on_first_line(&snap, bullet, "leaf bullet");
}

#[gpui::test]
fn sidebar_page_icon_centers_on_the_first_line_of_a_wrapped_row(cx: &mut TestAppContext) {
    let snap = render(cx, Marker::PageIcon, "page");
    let icons = of_type(&snap, "icon");
    let icon = icons.first().unwrap_or_else(|| {
        panic!(
            "the sidebar row must render its notebook icon:\n{}",
            snap.dump()
        )
    });
    assert_on_first_line(&snap, icon, "sidebar page icon");
}
