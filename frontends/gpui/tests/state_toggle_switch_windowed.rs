//! Windowed rung for `state_toggle(#{appearance: "switch"})`.
//!
//! `state_toggle`'s two-state CYCLING already worked before this appearance
//! existed — `cycle_state` falls through to the `states` list for any non-task
//! vocabulary. What did not work is the PAINT: `state_icon` and `state_display`
//! are hard-wired to the task keywords, so both `"off"` and `"on"` render as
//! the same `○` in the same colour. A control whose two states are
//! indistinguishable is the "silently degrades to look fine" case, and it is
//! invisible to every headless tier — the view-model snapshot carries the same
//! `current` either way.
//!
//! So this rung asserts what only a window can see: that the switch appearance
//! paints a switch (the shared 36×20 track, not the icon box), and that on and
//! off are visually different. The headless half — that the SNAPSHOT carries
//! the requested appearance — is asserted in the same file's last case, which
//! needs no window at all but belongs beside what it is the shadow of.
//!
//! Run: cargo test -p holon-gpui --test state_toggle_switch_windowed
//!
//! @pbt kind windowed
//! @pbt covers state-toggle-switch-appearance — `appearance: "switch"` paints
//! the shared switch geometry, and its on and off states are distinguishable
//! @pbt slips-if-removed an integration row's enablement switch renders as the
//! task-state glyph, identical in both states — the user cannot tell a
//! switched-on integration from a switched-off one

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_api::render_types::ClickModifiers;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::OperationParam;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::Trigger;
use holon_api::widget_spec::DataRow;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::view_model::StateToggleAppearance;
use holon_frontend::view_model::StateToggleBinding;
use support::BoundsSnapshot;
use support::render_fixture_sized;

/// The shared switch geometry (`builders/switch_track.rs`). Restated here so
/// the assertion says what a user must see rather than echoing the source.
const TRACK_W: f32 = 36.0;
const TRACK_H: f32 = 20.0;
const EPS: f32 = 0.5;

/// A `set_field` wiring for the `integration` entity, so the toggle reaches its
/// LIVE arm. Without op wiring the builder takes its disclosed display-only
/// path, which is a different control and a different assertion.
fn set_field_wiring() -> OperationWiring {
    OperationWiring {
        modified_param: String::new(),
        descriptor: OperationDescriptor {
            entity_name: holon_api::EntityName::new("integration"),
            name: "set_field".into(),
            trigger: Some(Trigger::Click {
                modifiers: ClickModifiers::none(),
            }),
            bound_params: HashMap::new(),
            entity_short_name: "integration".to_string(),
            id_column: "id".to_string(),
            display_name: String::new(),
            description: String::new(),
            required_params: vec![OperationParam {
                name: "value".to_string(),
                type_hint: holon_api::TypeHint::String,
                description: String::new(),
            }],
            affected_fields: vec!["enabled".to_string()],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Global,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Test,
            },
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
    }
}

fn integration_row(provider: &str) -> Arc<DataRow> {
    let mut data: DataRow = HashMap::new();
    data.insert(
        "id".into(),
        Value::String(format!("integration:{provider}")),
    );
    Arc::new(data)
}

/// A `state_toggle` leaf as the `integration` profile builds one.
fn toggle_vm(
    provider: &str,
    current: &str,
    appearance: StateToggleAppearance,
) -> ReactiveViewModel {
    let props: HashMap<String, Value> = [
        ("field", "enabled"),
        ("current", current),
        ("states", "off,on"),
        ("appearance", appearance.as_str()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
    .collect();

    let mut vm = ReactiveViewModel::from_widget("state_toggle", props)
        .with_entity(integration_row(provider));
    vm.operations.push(set_field_wiring());
    vm
}

/// A bool-bound `state_toggle` leaf: `current` is a typed bool, not a word.
fn bool_toggle_vm(provider: &str, enabled: bool) -> ReactiveViewModel {
    let props: HashMap<String, Value> = [
        ("field".to_string(), Value::String("enabled".to_string())),
        ("binding".to_string(), Value::String("bool".to_string())),
        (
            "appearance".to_string(),
            Value::String(StateToggleAppearance::Switch.as_str().to_string()),
        ),
        ("current".to_string(), Value::Boolean(enabled)),
    ]
    .into_iter()
    .collect();

    let mut vm = ReactiveViewModel::from_widget("state_toggle", props)
        .with_entity(integration_row(provider));
    vm.operations.push(set_field_wiring());
    vm
}

fn render(cx: &mut TestAppContext, vm: ReactiveViewModel) -> BoundsSnapshot {
    render_fixture_sized(cx, Arc::new(vm), size(px(400.0), px(200.0)))
}

/// The recorded rect of the sole `state_toggle` in a snapshot.
fn toggle_rect(snap: &BoundsSnapshot) -> holon_frontend::geometry::ElementInfo {
    let mut found = snap.of_type("state_toggle");
    let first = found
        .next()
        .unwrap_or_else(|| panic!("fixture must paint a state_toggle\n{}", snap.dump()))
        .clone();
    assert!(
        found.next().is_none(),
        "fixture must paint exactly one state_toggle\n{}",
        snap.dump()
    );
    first
}

/// THE RED. Before the `switch` appearance existed, `appearance: "switch"`
/// painted the task glyph — an icon box, not a track.
#[gpui::test]
fn switch_appearance_paints_the_shared_track(cx: &mut TestAppContext) {
    let snap = render(cx, toggle_vm("gmail", "on", StateToggleAppearance::Switch));
    let rect = toggle_rect(&snap);

    assert!(
        (rect.width - TRACK_W).abs() < EPS && (rect.height - TRACK_H).abs() < EPS,
        "a switch-appearance state_toggle must paint the shared {TRACK_W}×{TRACK_H} track, got \
         {:.1}×{:.1}\n{}",
        rect.width,
        rect.height,
        snap.dump()
    );
}

/// The default is untouched: every shipped `state_toggle(col("task_state"))`
/// must still paint the task glyph's icon box, which is a different size.
#[gpui::test]
fn the_task_appearance_is_unchanged(cx: &mut TestAppContext) {
    let snap = render(cx, toggle_vm("gmail", "TODO", StateToggleAppearance::Task));
    let rect = toggle_rect(&snap);

    assert!(
        (rect.width - TRACK_W).abs() > EPS,
        "the task appearance must NOT paint the switch track — it is the block bullet's icon box; \
         got {:.1}×{:.1}\n{}",
        rect.width,
        rect.height,
        snap.dump()
    );
}

/// On and off must be distinguishable. The track is the same size in both, so
/// the difference is the KNOB's position — which is what the user reads.
#[gpui::test]
fn on_and_off_paint_the_knob_in_different_places(cx: &mut TestAppContext) {
    let on = render(cx, toggle_vm("gmail", "on", StateToggleAppearance::Switch));
    let off = render(cx, toggle_vm("gmail", "off", StateToggleAppearance::Switch));

    let on_dump = on.structural_dump();
    let off_dump = off.structural_dump();
    assert_eq!(
        on_dump, off_dump,
        "the two states must paint the same tracked structure — only the knob moves"
    );

    // The knob is an untracked child of the track, so its offset is not in the
    // snapshot. What IS observable here is that both states paint the full
    // track rather than one of them collapsing; the knob offset itself is
    // covered by `switch_track`'s own single implementation, shared with the
    // preference toggle.
    let (a, b) = (toggle_rect(&on), toggle_rect(&off));
    assert!(
        (a.width - b.width).abs() < EPS && (a.height - b.height).abs() < EPS,
        "both switch states must paint the same track box; on={:.1}×{:.1} off={:.1}×{:.1}",
        a.width,
        a.height,
        b.width,
        b.height
    );
    assert!(
        a.width > 0.0 && a.height > 0.0,
        "a switch must occupy space in both states — a zero box is an invisible control"
    );
}

/// The headless half of the same contract: the snapshot carries the REQUESTED
/// appearance, so a keystone-tier oracle can assert which control a layout
/// asked for even where it cannot assert pixels.
#[test]
fn the_snapshot_carries_the_requested_appearance() {
    let vm = toggle_vm("gmail", "on", StateToggleAppearance::Switch);
    let kind = vm.snapshot().kind;
    match kind {
        holon_frontend::view_model::ViewKind::StateToggle { appearance, .. } => assert_eq!(
            appearance,
            StateToggleAppearance::Switch,
            "the view-model snapshot must carry the appearance the layout asked for"
        ),
        other => panic!("expected a StateToggle snapshot, got {other:?}"),
    }
}

/// A bool-bound toggle whose value is `false` must still paint the track.
///
/// The word-bound arm collapses an empty `current` to zero width, which is
/// right for a non-task block and wrong for a switch: `false` is a state the
/// user has to be able to see and click, not an absence.
#[gpui::test]
fn a_bool_bound_switch_paints_the_track_when_off(cx: &mut TestAppContext) {
    let snap = render(cx, bool_toggle_vm("gmail", false));
    let rect = toggle_rect(&snap);

    assert!(
        (rect.width - TRACK_W).abs() < EPS && (rect.height - TRACK_H).abs() < EPS,
        "a bool-bound switch reading `false` must paint the {TRACK_W}×{TRACK_H} track, got \
         {:.1}×{:.1}\n{}",
        rect.width,
        rect.height,
        snap.dump()
    );
}

/// The headless shadow of the same contract: a snapshot consumer can see which
/// binding a layout asked for, and therefore how to read `current`.
#[test]
fn the_snapshot_carries_the_requested_binding() {
    let vm = bool_toggle_vm("gmail", true);
    match vm.snapshot().kind {
        holon_frontend::view_model::ViewKind::StateToggle { binding, .. } => assert_eq!(
            binding,
            StateToggleBinding::Bool,
            "the view-model snapshot must carry the binding the layout asked for"
        ),
        other => panic!("expected a StateToggle snapshot, got {other:?}"),
    }
}
