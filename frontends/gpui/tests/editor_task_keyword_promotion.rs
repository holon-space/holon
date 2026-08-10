//! Windowed rung for the editable surface's SOURCE PROJECTION (task #78,
//! arm (d)).
//!
//! The headless keystone pins the STORAGE effect of typing `TODO ` (the block
//! gains `task_state` and loses the keyword from its content). It cannot pin
//! the half the user actually sees: what a real GPUI `InputState` DISPLAYS, and
//! where the caret lands in it. Under arm (d) the surface shows the block's
//! vault syntax — the keyword stays visible and editable — and the commit
//! routes to `set_field("source_text")`, where the STORE's parse is the only
//! thing that reads a keyword.
//!
//! These tests type through `replace_text_in_range`, the same platform entry
//! point the OS calls for a real keystroke, into a real `EditorView` in a real
//! window, in BOTH the no-cell (SqlOnly) and the cell-attached (Loro) arm.
//!
//! Run: cargo test -p holon-gpui --test editor_task_keyword_promotion

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::EntityInputHandler;
use gpui::TestAppContext;
use gpui::VisualTestContext;
use gpui::prelude::*;
use holon_api::Value;
use holon_api::widget_spec::DataRow;
use holon_frontend::reactive::BuilderServices;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::views::EditorView;
use support::TestServices;

const ROW_ID: &str = "block:task-keyword-promotion-test";

fn row(content: &str) -> Arc<DataRow> {
    let mut m = DataRow::new();
    m.insert("content".to_string(), Value::String(content.to_string()));
    Arc::new(m)
}

/// A row for a block that is ALREADY a task — the projection column that also
/// renders the row's task affordance.
fn tasked_row(content: &str, keyword: &str) -> Arc<DataRow> {
    let mut m = DataRow::new();
    m.insert("content".to_string(), Value::String(content.to_string()));
    m.insert("task_state".to_string(), Value::String(keyword.to_string()));
    Arc::new(m)
}

fn mount_editor<'a>(
    cx: &'a mut TestAppContext,
    data: &Mutable<Arc<DataRow>>,
) -> (
    gpui::Entity<EditorView>,
    &'a mut VisualTestContext,
    Arc<TestServices>,
) {
    mount_editor_with(cx, data, TestServices::new())
}

fn mount_editor_with<'a>(
    cx: &'a mut TestAppContext,
    data: &Mutable<Arc<DataRow>>,
    services_concrete: Arc<TestServices>,
) -> (
    gpui::Entity<EditorView>,
    &'a mut VisualTestContext,
    Arc<TestServices>,
) {
    cx.update(|cx| gpui_component::init(cx));
    let services: Arc<dyn BuilderServices> = services_concrete.clone();
    let data_handle = data.read_only();

    // The window's first layer MUST be a `gpui_component::Root`; smuggle the
    // real `EditorView` back out of the Root for assertions.
    let slot: Rc<RefCell<Option<gpui::Entity<EditorView>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let (_root, vcx) = cx.add_window_view(move |window, cx| {
        let editor = cx.new(|cx| {
            EditorView::new(
                "editor-el".to_string(),
                String::new(),
                "content".to_string(),
                ROW_ID.to_string(),
                Vec::new(),
                Vec::new(),
                services,
                NavigationState::new(),
                Some(data_handle),
                BoundsRegistry::new(),
                window,
                cx,
            )
        });
        *slot_for_build.borrow_mut() = Some(editor.clone());
        gpui_component::Root::new(editor, window, cx)
    });
    vcx.run_until_parked();
    let entity = slot
        .borrow()
        .clone()
        .expect("EditorView was built into the Root");
    (entity, vcx, services_concrete)
}

/// Type one character through the platform text-insertion entry point — the
/// same call the OS makes for a keystroke, so the whole
/// `InputEvent::Change` → `apply_local_edit` → dispatch path runs for real.
fn type_char(vcx: &mut VisualTestContext, entity: &gpui::Entity<EditorView>, ch: &str) {
    let input = entity.read_with(vcx, |ev, _| ev.input_entity().clone());
    input.update_in(vcx, |state, window, cx| {
        state.replace_text_in_range(None, ch, window, cx);
    });
    vcx.run_until_parked();
}

fn type_text(vcx: &mut VisualTestContext, entity: &gpui::Entity<EditorView>, text: &str) {
    for ch in text.chars() {
        type_char(vcx, entity, &ch.to_string());
    }
}

fn visible(vcx: &mut VisualTestContext, entity: &gpui::Entity<EditorView>) -> String {
    entity.read_with(vcx, |ev, cx| ev.input_entity().read(cx).value().to_string())
}

fn caret(vcx: &mut VisualTestContext, entity: &gpui::Entity<EditorView>) -> usize {
    entity.read_with(vcx, |ev, cx| ev.input_entity().read(cx).cursor())
}

// ── The editable surface ─────────────────────────────────────────────────

/// PRIMARY RUNG. Typing a leading keyword commits the WHOLE raw text through
/// the source channel and leaves it on screen. The old behaviour — strip the
/// keyword out of the visible field — is what produced the doubling shape: the
/// chip rendered `TODO` in front of a field the next focus re-seeded with
/// `TODO milk` again.
#[gpui::test]
fn typing_a_keyword_commits_the_source_and_keeps_it_visible(cx: &mut TestAppContext) {
    let data = Mutable::new(row(""));
    let (entity, vcx, services) = mount_editor(cx, &data);

    type_text(vcx, &entity, "TODO milk");

    assert_eq!(
        visible(vcx, &entity),
        "TODO milk",
        "the surface shows vault syntax; the keyword is text the user can edit"
    );
    assert_eq!(
        caret(vcx, &entity),
        9,
        "the caret is where the user typed it"
    );
    let last = services
        .recorded_intents()
        .into_iter()
        .last()
        .expect("the keystroke commits");
    assert_eq!(last.op_name, "set_field");
    assert_eq!(
        last.params["field"],
        Value::String(holon_api::SOURCE_TEXT_FIELD.into()),
        "keyword-headed text commits on the SOURCE channel, where the store parses it"
    );
    assert_eq!(last.params["value"], Value::String("TODO milk".into()));
    assert!(
        services
            .recorded_intents()
            .iter()
            .all(|i| i.op_name != "promote_task_keyword"),
        "the promotion compound is gone; recorded: {:?}",
        services.recorded_intents()
    );
}

/// Ordinary prose never touches the source channel — it commits `content`,
/// which by contract never re-derives the task state (the #64 lock).
#[gpui::test]
fn ordinary_prose_commits_the_content_channel(cx: &mut TestAppContext) {
    let data = Mutable::new(row(""));
    let (entity, vcx, services) = mount_editor(cx, &data);

    type_text(vcx, &entity, "milk");

    let last = services
        .recorded_intents()
        .into_iter()
        .last()
        .expect("the keystroke commits");
    assert_eq!(last.params["field"], Value::String("content".into()));
    assert_eq!(visible(vcx, &entity), "milk");
}

/// FOCUS SEED + CARET MAPPING (Inc 2). An editor mounted on a block that is
/// already a task shows its vault syntax, and a caret placed against the
/// DISPLAYED content crosses the keyword prefix with it — without the mapping a
/// mid-word click lands `keyword.len() + 1` bytes to the left.
#[gpui::test]
fn focus_seeds_the_keyword_and_maps_the_caret(cx: &mut TestAppContext) {
    let data = Mutable::new(row("milk"));
    let (entity, vcx, _services) = mount_editor(cx, &data);

    // Click mid-word in the displayed `milk`, between `mi` and `lk`.
    let input = entity.read_with(vcx, |ev, _| ev.input_entity().clone());
    input.update_in(vcx, |state, window, cx| {
        use gpui_component::input::RopeExt;
        state.set_value("milk", window, cx);
        let pos = state.text().offset_to_position(2);
        state.set_cursor_position(pos, window, cx);
    });
    vcx.run_until_parked();

    // The block becomes a task under the open editor and the surface re-seeds.
    data.set(tasked_row("milk", "TODO"));
    vcx.run_until_parked();

    assert_eq!(
        visible(vcx, &entity),
        "TODO milk",
        "the surface must show the block's vault syntax once it is a task"
    );
    assert_eq!(
        caret(vcx, &entity),
        7,
        "the caret still sits between `mi` and `lk` — it crossed the keyword prefix"
    );
}

/// DELETING the keyword is the demotion gesture and must reach the only channel
/// that can clear a task state. A content write here would leave the block a
/// task whose keyword the user can no longer see.
#[gpui::test]
fn deleting_the_keyword_commits_the_source_channel(cx: &mut TestAppContext) {
    let data = Mutable::new(tasked_row("milk", "TODO"));
    let (entity, vcx, services) = mount_editor(cx, &data);
    vcx.run_until_parked();

    let input = entity.read_with(vcx, |ev, _| ev.input_entity().clone());
    input.update_in(vcx, |state, window, cx| {
        state.replace_text_in_range(Some(0..5), "", window, cx);
    });
    vcx.run_until_parked();

    assert_eq!(visible(vcx, &entity), "milk");
    let last = services
        .recorded_intents()
        .into_iter()
        .last()
        .expect("the deletion commits");
    assert_eq!(
        last.params["field"],
        Value::String(holon_api::SOURCE_TEXT_FIELD.into()),
        "a buffer that STOPPED being keyword-headed must reach the demoting channel"
    );
    assert_eq!(last.params["value"], Value::String("milk".into()));
}

/// The staleness case in a window: the row gains `task_state` under an OPEN
/// editor (a task-toggle click, a peer, an agent). The surface is a projection
/// of the LIVE row, so the keyword appears — a value remembered from mount
/// would leave the user editing a surface that no longer describes the block.
#[gpui::test]
fn a_task_state_that_arrives_under_an_open_editor_is_shown(cx: &mut TestAppContext) {
    let data = Mutable::new(row("milk"));
    let (entity, vcx, _services) = mount_editor(cx, &data);

    data.set(tasked_row("milk", "TODO"));
    vcx.run_until_parked();

    assert_eq!(visible(vcx, &entity), "TODO milk");
}

/// THE PROD RE-PROJECTION EDGE, pinned. `EditorView` resolves the owning
/// document's vocabulary asynchronously and, when it lands, re-projects the
/// surface and converges — `converge_to("vocabulary_resolved", …)`. That edge
/// is the ONLY thing that closes the window in production: the `task_state`
/// signal is `dedupe_cloned`, so for an idle editor it fires once at mount
/// (while the vocabulary is still unresolved) and never again.
///
/// The window is held open BY THE TEST rather than raced: the fixture's query
/// engine awaits a gate before answering, so "unresolved" is a state this rung
/// can observe and assert against, then close on demand.
#[gpui::test]
fn the_surface_reclassifies_when_the_vocabulary_resolves(cx: &mut TestAppContext) {
    let (engine, release) = support::DeclaresNothingQueryEngine::gated();
    let data = Mutable::new(tasked_row("milk", "TODO"));
    let (entity, vcx, services) =
        mount_editor_with(cx, &data, TestServices::with_query_engine(Arc::new(engine)));

    // Window OPEN: every other signal has fired and settled, and the surface is
    // still unclassified — so it shows the content column, not vault syntax.
    assert_eq!(
        visible(vcx, &entity),
        "milk",
        "an unresolved vocabulary must not classify the surface"
    );

    // Close it.
    release
        .send(())
        .expect("the vocabulary read is still waiting");
    vcx.run_until_parked();

    assert_eq!(
        visible(vcx, &entity),
        "TODO milk",
        "REGRESSION: the vocabulary resolved but nothing re-projected the surface — the \
         feature silently disappears for the whole editing session"
    );
    // And the reclassification reaches the ROUTER, not just the pixels.
    type_text(vcx, &entity, "s");
    let last = services
        .recorded_intents()
        .into_iter()
        .last()
        .expect("the keystroke commits");
    assert_eq!(
        last.params["field"],
        Value::String(holon_api::SOURCE_TEXT_FIELD.into()),
        "a surface classified after the window closed must route as source"
    );
}

// ── Cell-attached (Loro / Full) arm ──────────────────────────────────────

/// A `Cell<String>` over an in-memory string — enough to make `EditorView`
/// take its cell-attached branch, which is the shipped Loro configuration and
/// the one where the per-row CONTENT subscription is deliberately dropped.
fn in_memory_cell(seed: &str) -> holon_core::cell::Cell<String> {
    let value = Arc::new(std::sync::Mutex::new(seed.to_string()));
    let read = value.clone();
    let write = value.clone();
    let backing = Arc::new(holon_core::cell::LwwTextCellBacking::new(
        Arc::new(move || read.lock().unwrap().clone()),
        Arc::new(move |new_value: String| {
            let v = write.clone();
            Box::pin(async move {
                *v.lock().unwrap() = new_value;
                Ok(())
            })
        }),
        Arc::new(|| Box::pin(futures::stream::empty())),
    ));
    holon_core::cell::Cell::from_backing(backing as Arc<dyn holon_core::cell::CellBacking<String>>)
}

fn mount_cell_editor<'a>(
    cx: &'a mut TestAppContext,
    data: &Mutable<Arc<DataRow>>,
    cell_seed: &str,
) -> (
    gpui::Entity<EditorView>,
    &'a mut VisualTestContext,
    Arc<TestServices>,
) {
    cx.update(|cx| gpui_component::init(cx));
    let services_concrete = TestServices::with_editable_cell(in_memory_cell(cell_seed));
    let services: Arc<dyn BuilderServices> = services_concrete.clone();
    let data_handle = data.read_only();

    let slot: Rc<RefCell<Option<gpui::Entity<EditorView>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let (_root, vcx) = cx.add_window_view(move |window, cx| {
        let editor = cx.new(|cx| {
            EditorView::new(
                "editor-el".to_string(),
                String::new(),
                "content".to_string(),
                ROW_ID.to_string(),
                Vec::new(),
                Vec::new(),
                services,
                NavigationState::new(),
                Some(data_handle),
                BoundsRegistry::new(),
                window,
                cx,
            )
        });
        *slot_for_build.borrow_mut() = Some(editor.clone());
        gpui_component::Root::new(editor, window, cx)
    });
    vcx.run_until_parked();
    let entity = slot
        .borrow()
        .clone()
        .expect("EditorView was built into the Root");
    assert!(
        entity.read_with(vcx, |ev, _| ev.has_cell()),
        "fixture precondition: this editor must be CELL-ATTACHED"
    );
    (entity, vcx, services_concrete)
}

/// THE CELL-ARM RUNG. A cell-attached editor drops its per-row CONTENT
/// subscription (the CRDT owns content), so `task_state` — which has no second
/// source — must still reach it, or the surface silently stops being the
/// block's vault syntax in the shipped Loro configuration.
#[gpui::test]
fn a_cell_attached_editor_shows_the_blocks_vault_syntax(cx: &mut TestAppContext) {
    let data = Mutable::new(tasked_row("milk", "TODO"));
    let (entity, vcx, _services) = mount_cell_editor(cx, &data, "milk");
    vcx.run_until_parked();

    assert_eq!(
        visible(vcx, &entity),
        "TODO milk",
        "REGRESSION: the cell arm lost sight of task_state, so the surface shows \
         the content column instead of the source projection"
    );
}

/// The cell arm routes a keyword-headed buffer through the SOURCE channel
/// rather than splicing it into the CRDT: the cell holds the CONTENT column,
/// which is not what the buffer says.
#[gpui::test]
fn a_cell_attached_editor_commits_keyword_headed_text_as_source(cx: &mut TestAppContext) {
    let data = Mutable::new(row(""));
    let (entity, vcx, services) = mount_cell_editor(cx, &data, "");

    type_text(vcx, &entity, "TODO milk");

    assert_eq!(visible(vcx, &entity), "TODO milk");
    let last = services
        .recorded_intents()
        .into_iter()
        .last()
        .expect("the cell arm must dispatch a source commit, not only a CRDT delta");
    assert_eq!(
        last.params["field"],
        Value::String(holon_api::SOURCE_TEXT_FIELD.into())
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
