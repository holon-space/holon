//! Green-from-start regression lock for the row-scoped hover-reveal chevron
//! (behavior landed via the row-scoped-hover-chevron work, cb1d0e7e family).
//!
//! The disclosure chevron on a tree row is HIDDEN until the row is hovered and
//! revealed on hover — but the reveal must be pure opacity, never layout. This
//! windowed test renders the SAME chevron row twice, once with `hovered=false`
//! and once with `hovered=true`, and asserts every element's geometry is
//! byte-identical across the two states:
//!
//!   * the chevron (`expand_toggle`) bounds do not move — locks constraint (a)
//!     (the leading-marker gutter stays reserved) and (b) (no hover re-flow);
//!   * the row's content text bounds do not move — the RED-FIRST "text doesn't
//!     shift" assertion.
//!
//! The opacity VALUE (0.0 hidden / 1.0 revealed) is locked by the pure-fn unit
//! tests on `chevron_reveal_opacity` in `builders/tree_item.rs` — `ElementInfo`
//! records geometry, not paint opacity, so opacity is asserted there and layout
//! invariance is asserted here. Together they cover the three lock points.
//!
//! Run: `cargo test -p holon-gpui --test chevron_hover_reveal_lock`

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::TestAppContext;
use holon_api::Value;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use support::BoundsSnapshot;
use support::render_fixture;

/// A production-shaped parent tree row: `has_children=true` (draws a chevron),
/// an explicit `target_id` so the chevron registers in the bounds registry
/// under `expand_toggle_id_for(target_id)`, one text content child, and a
/// seeded `hovered` cell in the requested state.
fn chevron_row(hovered: bool) -> Arc<ReactiveViewModel> {
    let mut props = HashMap::new();
    props.insert("depth".to_string(), Value::Float(0.0));
    props.insert("has_children".to_string(), Value::Boolean(true));
    props.insert("target_id".to_string(), Value::String("lock-row".to_string()));

    let content = ReactiveViewModel::text("Row content that must not reflow");
    let mut vm = ReactiveViewModel::from_widget("tree_item", props).with_children(vec![content]);
    vm.expanded = Some(Mutable::new(true));
    vm.hovered = Some(Mutable::new(hovered));
    Arc::new(vm)
}

/// The chevron's recorded geometry `(x, y, width, height)`. Asserts exactly one
/// `expand_toggle` element rendered so the comparison can never pass vacuously.
fn chevron_bounds(snap: &BoundsSnapshot) -> (f32, f32, f32, f32) {
    let chevrons: Vec<_> = snap.of_type("expand_toggle").collect();
    assert_eq!(
        chevrons.len(),
        1,
        "expected exactly one chevron (expand_toggle) element, got {}:\n{}",
        chevrons.len(),
        snap.dump()
    );
    let c = chevrons[0];
    (c.x, c.y, c.width, c.height)
}

/// The row's text content geometry `(x, y, width, height)`. Asserts exactly one
/// `text` element rendered so the comparison can never pass vacuously.
fn content_bounds(snap: &BoundsSnapshot) -> (f32, f32, f32, f32) {
    let texts: Vec<_> = snap.of_type("text").collect();
    assert_eq!(
        texts.len(),
        1,
        "expected exactly one text element, got {}:\n{}",
        texts.len(),
        snap.dump()
    );
    let c = texts[0];
    (c.x, c.y, c.width, c.height)
}

#[gpui::test]
fn chevron_hover_reveal_never_shifts_layout(cx: &mut TestAppContext) {
    let not_hovered = render_fixture(cx, chevron_row(false));
    let hovered = render_fixture(cx, chevron_row(true));

    // The chevron IS rendered in both states (hidden state keeps it in layout
    // via opacity, not a conditional render) and its bounds are identical —
    // hover reveals paint, never geometry. Locks constraint (a): the leading
    // gutter stays reserved whether the chevron is visible or transparent.
    assert_eq!(
        chevron_bounds(&not_hovered),
        chevron_bounds(&hovered),
        "chevron bounds must be identical hovered vs not — reveal is opacity, not layout"
    );

    // The row's content text must not move either (no re-flow on hover).
    assert_eq!(
        content_bounds(&not_hovered),
        content_bounds(&hovered),
        "row content must not reflow when the row is hovered"
    );
}
