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
use holon_frontend::reactive_view_model::ReactiveViewModel;

use support::render_fixture;

// ── Widget property helper ────────────────────────────────────────────

fn props(
    pairs: &[(&str, holon_api::Value)],
) -> std::collections::HashMap<String, holon_api::Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

// ── Fixture builders (reused across snapshot cases) ───────────────────

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

// ── Snapshots ─────────────────────────────────────────────────────────

#[gpui::test]
fn snapshot_simple_col(cx: &mut TestAppContext) {
    let fixture = column(vec![text("hello"), text("world")]);
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
    let fixture = column(vec![
        column(vec![text("a"), text("b")]),
        column(vec![text("c"), text("d")]),
    ]);
    let snap = render_fixture(cx, Arc::new(fixture));
    insta::assert_snapshot!(snap.structural_dump());
}
