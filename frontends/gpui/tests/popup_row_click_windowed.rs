//! Windowed rung for the slash-menu row CLICK path (task #45).
//!
//! THE BUG (Martin, GPUI dogfooding 2026-08-17): the slash menu paints its rows
//! but nothing happens when you click one — for EVERY command, not just
//! `embed_entity`. `render_popup` builds each row as a bare `div()` with no
//! mouse handler at all, so the only way to run a command is the keyboard
//! Enter path. A pointer user has no way to fire a slash command.
//!
//! The contract this rung pins: clicking a popup row behaves exactly like
//! pressing Enter on that row — the SAME `PopupMenu::select_current` →
//! `PopupResult` → `EditorAction` route, not a second dispatch path. The
//! observable that separates the two worlds is the editor buffer: an executed
//! slash command strips its own typed text (`ExecuteAndStripCommand`), so a
//! wired click leaves the block empty while a dead click leaves `/del` sitting
//! in the content.
//!
//! Run: cargo test -p holon-gpui --test popup_row_click_windowed

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::Focusable;
use gpui::TestAppContext;
use gpui::VisualTestContext;
use gpui::prelude::*;
use gpui::px;
use holon_api::EntityName;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::OperationParam;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::TypeHint;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::input_trigger::InputTrigger;
use holon_frontend::reactive::BuilderServices;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::views::EditorView;
use support::TestServices;

const ROW_ID: &str = "block:popup-click-test";

/// A slash-menu-exposed operation whose only required param is `id` — the
/// editor's context already carries it, so the operation is fully satisfied and
/// a pick executes immediately (no follow-up picker phase).
fn op(name: &str, display: &str, params: Vec<OperationParam>) -> OperationWiring {
    OperationWiring {
        modified_param: String::new(),
        descriptor: OperationDescriptor {
            entity_name: EntityName::new("block"),
            entity_short_name: "block".into(),
            name: name.into(),
            display_name: display.into(),
            required_params: params,
            id_column: "id".to_string(),
            description: String::new(),
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::Listed {
                surfaces: holon_api::SurfaceSet {
                    slash_menu: true,
                    action_bar: false,
                },
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
    }
}

fn param(name: &str, hint: TypeHint) -> OperationParam {
    OperationParam {
        name: name.into(),
        type_hint: hint,
        description: String::new(),
    }
}

fn operations() -> Vec<OperationWiring> {
    vec![op("delete", "Delete", vec![param("id", TypeHint::String)])]
}

/// The production slash trigger (`render/builders`): `/` anywhere on the line,
/// gated on a word boundary so URLs don't open the menu.
fn triggers() -> Vec<InputTrigger> {
    vec![InputTrigger::TextPrefix {
        prefix: "/".into(),
        action: "command_menu".into(),
        at_line_start: false,
        word_boundary: true,
    }]
}

struct Rig {
    editor: gpui::Entity<EditorView>,
    services: Arc<TestServices>,
    bounds: BoundsRegistry,
}

fn mount(cx: &mut TestAppContext) -> (Rig, &mut VisualTestContext) {
    cx.update(|cx| gpui_component::init(cx));
    let services =
        TestServices::with_registry_quiescent(Arc::new(support::BlockTreeRegistry::new()));
    let services_dyn: Arc<dyn BuilderServices> = services.clone();
    let bounds = BoundsRegistry::new();
    let bounds_for_view = bounds.clone();

    let slot: Rc<RefCell<Option<gpui::Entity<EditorView>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let (_root, vcx) = cx.add_window_view(move |window, cx| {
        let editor = cx.new(|cx| {
            EditorView::new(
                "editor-el".to_string(),
                String::new(),
                "content".to_string(),
                ROW_ID.to_string(),
                operations(),
                triggers(),
                services_dyn,
                NavigationState::new(),
                None,
                bounds_for_view,
                window,
                cx,
            )
        });
        *slot_for_build.borrow_mut() = Some(editor.clone());
        gpui_component::Root::new(editor, window, cx)
    });
    let editor = slot.borrow().clone().expect("EditorView built into Root");
    (
        Rig {
            editor,
            services,
            bounds,
        },
        vcx,
    )
}

fn focus_editor(vcx: &mut VisualTestContext, rig: &Rig) {
    let editor = rig.editor.clone();
    vcx.update(|window, cx| {
        let handle = editor.read(cx).input_entity().read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    });
    vcx.run_until_parked();
}

fn type_text(vcx: &mut VisualTestContext, text: &str) {
    let per_char: String = text
        .chars()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    vcx.simulate_keystrokes(&per_char);
    vcx.run_until_parked();
}

fn buffer(vcx: &mut VisualTestContext, rig: &Rig) -> String {
    let editor = rig.editor.clone();
    vcx.update(|_window, cx| editor.read(cx).input_entity().read(cx).value().to_string())
}

/// Centre of the popup row registered for `item_id`, or `None` when no such row
/// painted. Deliberately tolerant so a missing row fails the ASSERTION rather
/// than the scaffolding.
fn popup_row_center(rig: &Rig, item_id: &str) -> Option<gpui::Point<gpui::Pixels>> {
    rig.bounds.flush();
    let el_id = format!("popup-item-{item_id}");
    rig.bounds
        .all_elements()
        .into_iter()
        .find(|(id, _)| *id == el_id)
        .map(|(_, i)| gpui::point(px(i.x + i.width / 2.0), px(i.y + i.height / 2.0)))
}

/// THE RUNG. Run `delete` from the slash menu twice in one window — once with
/// the keyboard, once with the mouse — and require the two to agree.
///
/// The observable is the editor buffer. An executed slash command strips its
/// own typed text (`EditorAction::ExecuteAndStripCommand`), so a live pick
/// leaves the block empty. Pre-fix the keyboard leg passed and the mouse leg
/// failed with `/del` still in the content: the rows carried no mouse handler
/// at all.
///
/// The dispatched intent itself is NOT asserted: `ExecuteAndStripCommand` hands
/// the op to `services.runtime_handle()`, whose thread gpui's test scheduler
/// does not drive. The strip runs on the gpui executor in the same ordered
/// spawn, so it is the deterministic half of that pair — and it is exactly the
/// half that separates a live pick from a dead one.
///
/// Both gestures share ONE window on purpose: a second `add_window_view` in the
/// same process makes the fixture's shared tokio runtime trip gpui's
/// test-scheduler determinism assertion.
#[gpui::test]
fn clicking_a_popup_row_runs_the_command_just_like_enter(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    focus_editor(vcx, &rig);

    // Keyboard leg — the baseline the pointer must match.
    type_text(vcx, "/del");
    assert_eq!(
        buffer(vcx, &rig),
        "/del",
        "precondition: the typed command text is in the editor buffer"
    );
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    let by_enter = buffer(vcx, &rig);
    assert_eq!(
        by_enter, "",
        "precondition: Enter on the highlighted row executes the command and \
         strips its typed text"
    );

    // Pointer leg — same command, same row, mouse instead of Enter.
    type_text(vcx, "/del");
    let center = popup_row_center(&rig, "delete")
        .expect("precondition: the slash menu must paint a row for the `delete` command");
    vcx.simulate_mouse_move(center, None, Default::default());
    vcx.simulate_click(center, Default::default());
    vcx.run_until_parked();

    assert_eq!(
        buffer(vcx, &rig),
        by_enter,
        "REGRESSION: clicking a slash-menu row left the editor in a different \
         state than pressing Enter on it. Popup rows must carry a mouse handler \
         that routes through the SAME PopupMenu::select_current path the Enter \
         key uses — not a second dispatch path, and not nothing at all."
    );
}
