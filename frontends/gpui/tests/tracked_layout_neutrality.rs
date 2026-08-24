//! The `tracked()` layout-neutrality contract.
//!
//! @pbt oracle internal-consistency
//! @pbt covers bounds-tracking-is-observation — a `tracked()` wrapper records
//!   the geometry its child was laid out with, and contributes none of its own
//! @pbt slips-if-removed an observability wrapper that injects layout style
//!   silently resizes shipped rows: the bullet's click region claims half the
//!   row (caret placement dead there) and the block wrapper collapses to
//!   zero height
//!
//! The element `geometry::tracked()` returns (a `TransparentTracker`) is a
//! pure observer: its job is to read its child's post-layout rect. If it also
//! *contributes* layout, every recorded rect is a measurement of the
//! instrument rather than the subject.
//!
//! The fixture is the shipped main-panel block row, HAND-TRANSCRIBED from
//! `assets/default/types/block_profile.yaml` (not loaded from it — the profile
//! is not parsed here; keep this in sync if that render string changes):
//!
//! ```text
//! row(#{gap: 2, align: "start"}, selectable(icon(...)), rendered_text(content))
//! ```
//!
//! Both `selectable` (`builders/selectable.rs`) and `rendered_text`
//! (`builders/rendered_text.rs`) go through `tracked()`, so they are two
//! tracked siblings of one flex row — the exact shape the 2026-08-03 dogfood
//! capture found split 420/420 across an 844px row.
//!
//! The properties compare each wrapper against geometry recorded elsewhere in
//! the same snapshot (the icon's own `tag()` rect, the sibling's origin), so
//! there are no pixel constants to re-tune when the theme changes.

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_api::render_types::ClickModifiers;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::Trigger;
use holon_api::widget_spec::DataRow;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;
use support::BoundsSnapshot;
use support::render_fixture_sized;

/// Sub-pixel rounding slack, matching `size_expectation`'s tolerance.
const EPS: f32 = 0.5;

/// The `gap` the shipped row profile declares.
const ROW_GAP: f32 = 2.0;

// ── The shipped block-row fixture ──────────────────────────────────────

fn props(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn data_row(id: &str) -> Arc<DataRow> {
    let mut data: DataRow = HashMap::new();
    data.insert("id".into(), Value::String(id.into()));
    Arc::new(data)
}

/// `selectable` returns its child unwrapped when no click-triggered operation
/// is bound, so the fixture must carry real wiring to reach `tracked()` at
/// all. Mirrors the `focus` binding the shipped profile puts on the bullet.
fn click_wiring() -> OperationWiring {
    OperationWiring {
        modified_param: String::new(),
        descriptor: OperationDescriptor {
            entity_name: holon_api::EntityName::new("navigation"),
            name: "focus".into(),
            trigger: Some(Trigger::Click {
                modifiers: ClickModifiers::none(),
            }),
            bound_params: HashMap::new(),
            entity_short_name: String::new(),
            id_column: "id".to_string(),
            display_name: String::new(),
            description: String::new(),
            required_params: vec![],
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Test,
            },
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
    }
}

fn block_row_vm(row_id: &str, content: &str, icon_size: f32) -> ReactiveViewModel {
    let bullet = ReactiveViewModel::from_widget(
        "icon",
        props(&[
            ("name", Value::String("circle".into())),
            ("size", Value::Float(icon_size as f64)),
        ]),
    );

    let mut selectable = ReactiveViewModel::from_widget("selectable", HashMap::new())
        .with_entity(data_row(row_id))
        .with_children(vec![bullet]);
    selectable.operations.push(click_wiring());

    let text = ReactiveViewModel::from_widget(
        "rendered_text",
        props(&[
            ("content", Value::String(content.into())),
            ("field", Value::String("content".into())),
        ]),
    )
    .with_entity(data_row(row_id));

    ReactiveViewModel::from_widget(
        "row",
        props(&[
            ("gap", Value::Float(ROW_GAP as f64)),
            ("align", Value::String("start".into())),
        ]),
    )
    .with_children(vec![selectable, text])
}

// ── Snapshot lookups ───────────────────────────────────────────────────

fn by_id<'a>(
    snap: &'a BoundsSnapshot,
    el_id: &str,
) -> Option<&'a holon_frontend::geometry::ElementInfo> {
    snap.entries
        .iter()
        .find(|(id, _)| id == el_id)
        .map(|(_, info)| info)
}

/// The bullet's own rect, recorded by the `tag()` transparent tracker that
/// wraps every builder. This is what the `selectable` wrapper is supposed to
/// be measuring, and therefore the yardstick the wrapper is compared against.
fn icon_rect(snap: &BoundsSnapshot) -> Option<&holon_frontend::geometry::ElementInfo> {
    snap.of_type("icon").next()
}

// ── The properties ─────────────────────────────────────────────────────

/// Checks one laid-out row. `Err` carries the sub-check name plus the full
/// snapshot dump so a shrunk counterexample is directly readable.
fn check_row(snap: &BoundsSnapshot, row_id: &str, label: &str) -> Result<(), String> {
    let fail = |sub: &str, detail: String| {
        Err(format!(
            "[tracked-layout-neutral/{sub}] {label}: {detail}\n{}",
            snap.dump()
        ))
    };

    let Some(sel) = by_id(snap, &format!("selectable-{row_id}")) else {
        return fail(
            "fixture-renders",
            "no `selectable-` element recorded — the fixture did not reach tracked()".into(),
        );
    };
    let Some(icon) = icon_rect(snap) else {
        return fail("fixture-renders", "no `icon` element recorded".into());
    };
    let Some(text) = by_id(snap, &format!("rendered-text-{row_id}-content")) else {
        return fail(
            "fixture-renders",
            "no `rendered-text-` element recorded".into(),
        );
    };

    // interaction-region-content-sized: the click region a user aims at is the
    // bullet. A wrapper that claims the whole row makes every click in the
    // left half hit the bullet instead of the text.
    if sel.width > icon.width + EPS {
        return fail(
            "interaction-region-content-sized",
            format!(
                "selectable is {:.1}px wide but the bullet it wraps is only {:.1}px — the \
                 wrapper contributed width of its own",
                sel.width, icon.width
            ),
        );
    }

    // interaction-region-non-degenerate: a zero-height rect is unclickable,
    // and its "centre" resolves onto a neighbouring row.
    if sel.height < icon.height - EPS {
        return fail(
            "interaction-region-non-degenerate",
            format!(
                "selectable is {:.1}px tall but the bullet it wraps is {:.1}px",
                sel.height, icon.height
            ),
        );
    }

    // text-follows-bullet: with `align: start` and `gap: 2` the content starts
    // immediately after the bullet. Displacement here is what puts the caret
    // out of reach on the left half of the row.
    let expected_x = sel.x + sel.width + ROW_GAP;
    if text.x > expected_x + EPS {
        return fail(
            "text-follows-bullet",
            format!(
                "rendered_text starts at x={:.1} but the bullet ends at x={:.1} (+gap {ROW_GAP}) \
                 — the text was displaced",
                text.x,
                sel.x + sel.width
            ),
        );
    }

    Ok(())
}

fn strategy() -> impl Strategy<Value = (f32, f32, String)> {
    (
        // Window width — the row's available space.
        (400u32..1400u32).prop_map(|w| w as f32),
        // Bullet size, as `block_profile.yaml` parameterises it.
        (8u32..24u32).prop_map(|s| s as f32),
        // Block content. Non-empty: an empty block takes `rendered_text`'s
        // placeholder branch, a different (and separately covered) shape.
        "[A-Za-z ]{1,40}".prop_filter("non-blank", |s| !s.trim().is_empty()),
    )
}

/// ★ The contract: `tracked()` observes layout, it does not contribute any.
#[gpui::test]
fn tracked_wrapper_contributes_no_layout(cx: &mut TestAppContext) {
    let cx = std::cell::RefCell::new(cx);
    let mut runner = TestRunner::new(Config {
        cases: 24,
        ..Config::default()
    });

    runner
        .run(&strategy(), |(width, icon_size, content)| {
            let row_id = "block:row-under-test";
            let label = format!("w={width} icon={icon_size} content={content:?}");
            let vm = Arc::new(block_row_vm(row_id, &content, icon_size));

            let snap = render_fixture_sized(&mut **cx.borrow_mut(), vm, size(px(width), px(600.0)));

            check_row(&snap, row_id, &label).map_err(TestCaseError::fail)
        })
        .expect("tracked() must be layout-neutral");
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
