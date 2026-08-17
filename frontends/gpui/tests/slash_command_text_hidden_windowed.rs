//! Windowed rung for D1.b (task #47): the typed slash-command text must not be
//! visible while a picker phase of that command is open.
//!
//! THE BUG (Martin, GPUI dogfooding 2026-08-17): type `/enti`, press Enter, and
//! the entity picker opens — but the literal `/enti` keeps sitting in the
//! editor block until an entity is picked. The user sees their command text
//! masquerading as block content for the whole duration of the pick.
//!
//! THE RULING: while ANY picker phase of a slash command is open, the typed
//! command text is hidden from the rendered block, for every entity-typed
//! command. Cancelling the picker (Escape) brings the text back VERBATIM — the
//! hiding is a display decision, never a destructive edit the user can lose
//! work to.
//!
//! Run: cargo test -p holon-gpui --test slash_command_text_hidden_windowed

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::Focusable;
use gpui::TestAppContext;
use gpui::VisualTestContext;
use gpui::prelude::*;
use holon_api::EntityName;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::OperationParam;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::TypeHint;
use holon_frontend::input_trigger::InputTrigger;
use holon_frontend::reactive::BuilderServices;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::views::EditorView;
use support::TestServices;

const ROW_ID: &str = "block:hidden-command-test";

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

/// `embed_entity` is the shape the ruling is about: its `target_uri` is an
/// `EntityId` the context cannot supply, so picking it opens a second phase
/// (the entity picker) instead of executing.
fn operations() -> Vec<OperationWiring> {
    vec![
        op("delete", "Delete", vec![param("id", TypeHint::String)]),
        op(
            "embed_entity",
            "Embed entity",
            vec![
                param("id", TypeHint::String),
                param(
                    "target_uri",
                    TypeHint::EntityId {
                        entity_name: EntityName::new("block"),
                    },
                ),
            ],
        ),
    ]
}

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
}

fn mount(cx: &mut TestAppContext) -> (Rig, &mut VisualTestContext) {
    cx.update(|cx| gpui_component::init(cx));
    let services =
        TestServices::with_registry_quiescent(Arc::new(support::BlockTreeRegistry::new()));
    let services_dyn: Arc<dyn BuilderServices> = services;

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
                BoundsRegistry::new(),
                window,
                cx,
            )
        });
        *slot_for_build.borrow_mut() = Some(editor.clone());
        gpui_component::Root::new(editor, window, cx)
    });
    let editor = slot.borrow().clone().expect("EditorView built into Root");
    (Rig { editor }, vcx)
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
        .map(|c| {
            if c == ' ' {
                "space".to_string()
            } else {
                c.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    vcx.simulate_keystrokes(&per_char);
    vcx.run_until_parked();
}

fn buffer(vcx: &mut VisualTestContext, rig: &Rig) -> String {
    let editor = rig.editor.clone();
    vcx.update(|_window, cx| editor.read(cx).input_entity().read(cx).value().to_string())
}

/// PRIMARY RUNG. Picking `Embed entity` opens the target picker; the typed
/// `/emb` must disappear from the block for as long as that picker is open.
#[gpui::test]
fn picker_phase_hides_the_typed_command_text(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    focus_editor(vcx, &rig);
    type_text(vcx, "/emb");
    assert_eq!(
        buffer(vcx, &rig),
        "/emb",
        "precondition: the typed command text is in the buffer before the pick"
    );

    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    assert_eq!(
        buffer(vcx, &rig),
        "",
        "REGRESSION: the entity picker is open but the typed command text is \
         still rendered in the block. While a picker phase of a slash command \
         is open, that text must be hidden."
    );
}

/// THE SAFETY HALF OF THE RULING. Cancelling the picker restores the typed text
/// verbatim — hiding is a display decision, not a destructive edit.
#[gpui::test]
fn escaping_the_picker_restores_the_typed_command_text(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    focus_editor(vcx, &rig);
    type_text(vcx, "/emb");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();

    assert_eq!(
        buffer(vcx, &rig),
        "/emb",
        "REGRESSION: escaping the picker did not bring the typed command text \
         back. The hidden text must be restored verbatim on cancel."
    );
}

/// Typing INTO the picker must keep the command text hidden — the search term
/// the user types is the only thing that appears.
#[gpui::test]
fn typing_a_search_term_keeps_the_command_text_hidden(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    focus_editor(vcx, &rig);
    type_text(vcx, "/emb");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    type_text(vcx, "proj");

    assert_eq!(
        buffer(vcx, &rig),
        "proj",
        "REGRESSION: the block shows the command text alongside the search term"
    );

    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert_eq!(
        buffer(vcx, &rig),
        "/embproj",
        "REGRESSION: cancelling after typing a search term must restore the \
         command text in front of what the user typed, leaving the line exactly \
         as it would have been with no hiding at all"
    );
}

/// Whether a menu is open, so a rung can assert MENU STATE and not merely the
/// buffer string. Read from the controller rather than the bounds registry,
/// which keeps the last frame that painted rows and would report a closed menu
/// as still open.
fn menu_open(vcx: &mut VisualTestContext, rig: &Rig) -> bool {
    let editor = rig.editor.clone();
    vcx.update(|_window, cx| editor.read(cx).is_popup_active())
}

/// KEYSTONE RUNG (verifier defect 1). A slash command typed ANYWHERE but column
/// 0 stores a hide-time `prefix_start` that no longer indexes the buffer once
/// the user edits across it. Backspacing out of the picker on `hello /emb`
/// leaves a 5-byte buffer and a restore that slices `&text[..6]` — a panic in
/// the editor.
///
/// The contract: backspace at the anchor CANCELS the picker phase and puts the
/// command text back, and the keystroke is consumed by that cancel rather than
/// also deleting a character.
///
/// This is also the only non-vacuous restore rung: with hiding disabled the
/// backspace simply eats the `b` and the buffer reads `hello /em`.
#[gpui::test]
fn backspacing_out_of_a_mid_line_picker_cancels_and_restores(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    focus_editor(vcx, &rig);
    type_text(vcx, "hello /emb");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(
        buffer(vcx, &rig),
        "hello ",
        "precondition: the mid-line command text is hidden while the picker is open"
    );

    vcx.simulate_keystrokes("backspace");
    vcx.run_until_parked();

    assert_eq!(
        buffer(vcx, &rig),
        "hello /emb",
        "REGRESSION: backspacing at the hide anchor must cancel the picker and \
         restore the command text verbatim. The stored prefix_start is a \
         hide-time coordinate — revalidate it against the live buffer instead \
         of slicing with it."
    );
    assert!(
        !menu_open(vcx, &rig),
        "the cancelled picker must leave no menu behind"
    );
}

/// VERIFIER DEFECT 2. The hide span was `abs_start..cursor`, so moving the
/// caret left before picking left the tail of the command visible. The span
/// must be the command the MENU matched on, not whatever the caret happens to
/// bracket.
#[gpui::test]
fn hiding_covers_the_whole_command_even_with_the_caret_moved_left(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    focus_editor(vcx, &rig);
    type_text(vcx, "/emb");
    // A caret move is not a text change, so the menu keeps its `emb` filter
    // while the cursor no longer sits at the end of the typed command.
    vcx.simulate_keystrokes("left");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    assert_eq!(
        buffer(vcx, &rig),
        "",
        "REGRESSION: the pick hid only the text before the caret and left the \
         command's tail in the block"
    );
}

/// VERIFIER DEFECT 4. The restore is a programmatic `set_value`, and the
/// one-shot that suppressed its change event assumed exactly one event fires.
/// Assert MENU STATE, not just the buffer: the cancelled menu stays closed, and
/// the very next typed trigger still opens a fresh one.
#[gpui::test]
fn a_cancelled_picker_leaves_the_trigger_working(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    focus_editor(vcx, &rig);
    type_text(vcx, "/emb");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();

    assert_eq!(
        buffer(vcx, &rig),
        "/emb",
        "precondition: Escape restored the text"
    );
    assert!(
        !menu_open(vcx, &rig),
        "REGRESSION: restoring the command text re-opened the menu the user \
         just escaped out of"
    );

    // The next real trigger must still be seen.
    type_text(vcx, " /del");
    assert!(
        menu_open(vcx, &rig),
        "REGRESSION: the trigger check was still suppressed on the next \
         keystroke — the restore suppression stayed armed and swallowed a real \
         trigger"
    );
}
