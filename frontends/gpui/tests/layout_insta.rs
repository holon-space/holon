//! Structural layout snapshot tests.
//!
//! For each fixture, render it through `render_fixture` and pin the result
//! with `insta::assert_snapshot!` against a `.snap` file under
//! `tests/snapshots/`. The snapshot content is the deterministic
//! `BoundsSnapshot::structural_dump` — an indented tree of
//! `widget_type WxH` lines with integer-rounded pixel dimensions.
//!
//! These catch structural regressions that the invariant tests can't:
//!   - a widget quietly disappearing (child count mismatch)
//!   - a container's shape changing (e.g. row → col refactor broke sizing)
//!   - subtle pixel-dimension regressions beyond the 1px rounding floor
//!
//! To accept a change: `cargo insta review -p holon-gpui`.
//! To update all: `cargo insta accept -p holon-gpui`.
//!
//! The snapshots live in `frontends/gpui/tests/snapshots/layout_insta__*.snap`
//! and are committed. First-run test invocations panic with "snapshot not
//! found" unless one is accepted.

mod support;

use std::sync::Arc;

use gpui::TestAppContext;
use holon_frontend::reactive_view_model::{ReactiveViewKind, ReactiveViewModel};

use support::render_fixture;

// ── Fixture builders (reused across snapshot cases) ───────────────────

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

// ── Snapshots ─────────────────────────────────────────────────────────

#[gpui::test]
fn snapshot_simple_col(cx: &mut TestAppContext) {
    let fixture = col(vec![text("hello"), text("world")]);
    let snap = render_fixture(cx, Arc::new(fixture));
    insta::assert_snapshot!(snap.structural_dump());
}

#[gpui::test]
fn snapshot_row_with_icon_text_badge(cx: &mut TestAppContext) {
    let fixture = row(vec![icon("info"), text("Heading"), badge("NEW")]);
    let snap = render_fixture(cx, Arc::new(fixture));
    insta::assert_snapshot!(snap.structural_dump());
}

#[gpui::test]
fn snapshot_card_with_header_and_body(cx: &mut TestAppContext) {
    let fixture = card(vec![
        row(vec![icon("info"), text("Title")]),
        text("Line one"),
        text("Line two"),
    ]);
    let snap = render_fixture(cx, Arc::new(fixture));
    insta::assert_snapshot!(snap.structural_dump());
}

#[gpui::test]
fn snapshot_nested_col_of_col(cx: &mut TestAppContext) {
    let fixture = col(vec![
        col(vec![text("a"), text("b")]),
        col(vec![text("c"), text("d")]),
    ]);
    let snap = render_fixture(cx, Arc::new(fixture));
    insta::assert_snapshot!(snap.structural_dump());
}
