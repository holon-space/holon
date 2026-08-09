//! Windowed rung for live task-keyword promotion (task #64 Inc 4).
//!
//! The headless keystone pins the STORAGE effect of typing `TODO ` (the block
//! gains `task_state` and loses the keyword from its content). It cannot pin
//! the half the user actually sees: a real GPUI `InputState` keeps showing the
//! text the platform inserted unless the adapter re-seeds it from the view
//! model. That re-seed (`EditorView::apply_buffer_rewrite`) is production code
//! no headless rung reaches, and without it the visible field reads
//! `TODO buy milk` while the row renders a TODO chip in front of `buy milk` —
//! the keyword doubled on screen.
//!
//! These tests type through `replace_text_in_range`, the same platform entry
//! point the OS calls for a real keystroke, into a real no-cell (SqlOnly)
//! `EditorView` in a real window.
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
    cx.update(|cx| gpui_component::init(cx));
    let services_concrete = TestServices::new();
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

/// PRIMARY RUNG. Typing the space that commits a leading `TODO` dispatches the
/// promotion compound AND takes the keyword out of the visible field, caret
/// following the surviving text.
#[gpui::test]
fn typing_the_keyword_promotes_and_clears_it_from_the_visible_field(cx: &mut TestAppContext) {
    let data = Mutable::new(row(""));
    let (entity, vcx, services) = mount_editor(cx, &data);

    type_text(vcx, &entity, "TODO ");

    let promotions: Vec<_> = services
        .recorded_intents()
        .into_iter()
        .filter(|i| i.op_name == "promote_task_keyword")
        .collect();
    assert_eq!(
        promotions.len(),
        1,
        "the committing space must dispatch exactly one promotion; recorded: {:?}",
        services.recorded_intents()
    );
    let intent = &promotions[0];
    assert_eq!(intent.entity_name, "block");
    assert_eq!(intent.params["id"], Value::String(ROW_ID.to_string()));
    assert_eq!(intent.params["typed"], Value::String("TODO ".to_string()));
    assert_eq!(intent.params["keyword"], Value::String("TODO".to_string()));
    assert!(
        matches!(intent.params.get("write_seq"), Some(Value::Integer(_))),
        "the compound must carry the editor's write_seq so its echo is recognised"
    );

    assert_eq!(
        visible(vcx, &entity),
        "",
        "REGRESSION: the keyword is now the block's task state, so the visible \
         field must not still show it — the row would render the keyword twice"
    );
    assert_eq!(caret(vcx, &entity), 0);
}

/// The caret keeps its place within the text that survives the strip: after
/// `TODO milk` the user is still typing at the end of `milk`, not 5 bytes past
/// it (which is not even a legal offset in the new text).
#[gpui::test]
fn the_caret_follows_the_stripped_text(cx: &mut TestAppContext) {
    let data = Mutable::new(row(""));
    let (entity, vcx, _services) = mount_editor(cx, &data);

    type_text(vcx, &entity, "TODO milk");

    assert_eq!(visible(vcx, &entity), "milk");
    assert_eq!(caret(vcx, &entity), 4);
}

/// Negative control: a bare keyword with no space is ordinary text. Nothing is
/// stripped and no promotion is dispatched — the visible field must be left
/// exactly as typed, or every `TODO`-shaped word would lose characters.
#[gpui::test]
fn a_bare_keyword_is_left_alone(cx: &mut TestAppContext) {
    let data = Mutable::new(row(""));
    let (entity, vcx, services) = mount_editor(cx, &data);

    type_text(vcx, &entity, "TODO");

    assert_eq!(visible(vcx, &entity), "TODO");
    assert_eq!(caret(vcx, &entity), 4);
    assert!(
        services
            .recorded_intents()
            .iter()
            .all(|i| i.op_name == "set_field"),
        "no promotion until the space commits it; recorded: {:?}",
        services.recorded_intents()
    );
}

/// THE TEXT-LOSS RUNG (BugFunnel 2026-08-10). Typing a keyword a SECOND time
/// into a block that is already a task must leave the text alone. The strip is
/// applied before the write is confirmed, so a promotion the engine would
/// refuse leaves the buffer short and the next keystroke commits the short text
/// over the engine's verbatim commit — the keyword silently deleted.
#[gpui::test]
fn a_second_keyword_after_a_promotion_is_ordinary_text(cx: &mut TestAppContext) {
    let data = Mutable::new(row(""));
    let (entity, vcx, services) = mount_editor(cx, &data);

    type_text(vcx, &entity, "TODO ");
    assert_eq!(
        visible(vcx, &entity),
        "",
        "precondition: the block promoted"
    );

    // There is no engine behind `TestServices` — it records intents — so the
    // row the compound would update is pushed by hand, through the same per-row
    // `Mutable` production feeds. DISCLOSED: prod updates that row
    // asynchronously, so a second keyword typed INSIDE that window still reads
    // a row that has not caught up (the lane report's "in-flight window").
    data.set(tasked_row("", "TODO"));
    vcx.run_until_parked();

    type_text(vcx, &entity, "TODO x");

    assert_eq!(
        visible(vcx, &entity),
        "TODO x",
        "REGRESSION: the block is already a task, so this keyword is text — \
         stripping it here deletes what the user typed"
    );
    let promotions = services
        .recorded_intents()
        .into_iter()
        .filter(|i| i.op_name == "promote_task_keyword")
        .count();
    assert_eq!(promotions, 1, "promotion is one-shot per block");
}

/// The mount seed. An editor opening on a block that is ALREADY a task must
/// learn that from its row, or the very first keyword it sees becomes the same
/// text-loss bug — this is the cross-session half, which no amount of
/// in-session bookkeeping can cover.
#[gpui::test]
fn an_editor_mounted_on_an_existing_task_does_not_promote(cx: &mut TestAppContext) {
    let data = Mutable::new(tasked_row("milk", "TODO"));
    let (entity, vcx, services) = mount_editor(cx, &data);

    type_text(vcx, &entity, "TODO ");

    assert_eq!(
        visible(vcx, &entity),
        "TODO ",
        "the block already carries TODO, so this text is not an authoring gesture"
    );
    assert!(
        services
            .recorded_intents()
            .iter()
            .all(|i| i.op_name == "set_field"),
        "no promotion may be proposed for an already-tasked block; recorded: {:?}",
        services.recorded_intents()
    );
}

/// The staleness case in a window: the row gains `task_state` under an OPEN
/// editor (a task-toggle click, a peer, an agent). The keyword is read at the
/// keystroke, so the guard sees the new state — a value remembered from mount
/// would be stale exactly here.
#[gpui::test]
fn a_task_state_that_arrives_under_an_open_editor_is_seen(cx: &mut TestAppContext) {
    let data = Mutable::new(row(""));
    let (entity, vcx, services) = mount_editor(cx, &data);

    data.set(tasked_row("", "TODO"));
    vcx.run_until_parked();

    type_text(vcx, &entity, "TODO ");

    assert_eq!(
        visible(vcx, &entity),
        "TODO ",
        "REGRESSION: the block became a task under this editor, so the keyword \
         is text — stripping it deletes what the user typed"
    );
    assert!(
        services
            .recorded_intents()
            .iter()
            .all(|i| i.op_name != "promote_task_keyword"),
        "recorded: {:?}",
        services.recorded_intents()
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
) -> (
    gpui::Entity<EditorView>,
    &'a mut VisualTestContext,
    Arc<TestServices>,
) {
    cx.update(|cx| gpui_component::init(cx));
    let services_concrete = TestServices::with_editable_cell(in_memory_cell(""));
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
/// subscription (the CRDT owns content), so anything that reads the row must
/// not travel through that handle. `task_state` has no second source: an
/// editor that cannot see it proposes a promotion on every already-tasked
/// block, from the first keystroke, and the engine's refused-but-verbatim
/// commit is then overwritten by the stripped buffer.
#[gpui::test]
fn a_cell_attached_editor_sees_the_blocks_task_state(cx: &mut TestAppContext) {
    let data = Mutable::new(tasked_row("milk", "TODO"));
    let (entity, vcx, services) = mount_cell_editor(cx, &data);

    type_text(vcx, &entity, "TODO ");

    assert_eq!(
        visible(vcx, &entity),
        "TODO ",
        "REGRESSION: the block already carries TODO, so this is text — a \
         cell-attached editor must read task_state just like a no-cell one"
    );
    assert!(
        services
            .recorded_intents()
            .iter()
            .all(|i| i.op_name != "promote_task_keyword"),
        "no promotion may be proposed for an already-tasked block; recorded: {:?}",
        services.recorded_intents()
    );
}

/// The cell arm still PROMOTES when it should — the guard reads the row, it
/// does not simply disable itself when a cell is attached.
#[gpui::test]
fn a_cell_attached_editor_still_promotes_a_plain_block(cx: &mut TestAppContext) {
    let data = Mutable::new(row(""));
    let (entity, vcx, services) = mount_cell_editor(cx, &data);

    type_text(vcx, &entity, "TODO ");

    let promotions = services
        .recorded_intents()
        .into_iter()
        .filter(|i| i.op_name == "promote_task_keyword")
        .count();
    assert_eq!(promotions, 1, "the cell arm promotes too");
    assert_eq!(visible(vcx, &entity), "");
}

// ── Refusal recovery ─────────────────────────────────────────────────────

/// THE RECOVERY RUNG. The strip is applied before the write is confirmed, and
/// the trigger's read is NOT transactional with the dispatch — nothing holds
/// the block between them, so a peer, an agent or a rule writing `task_state`
/// in that interval turns an accepted proposal into a refusal no matter how
/// fresh the read was. That race is not deterministically reproducible; the
/// behaviour it needs is, and this is it: on a refusal the keyword comes BACK.
///
/// Without this the same shape as the original bug returns — engine commits
/// `TODO milk` verbatim, editor shows `milk`, next keystroke overwrites the
/// engine's text with the stripped one.
#[gpui::test]
fn a_refused_promotion_puts_the_keyword_back(cx: &mut TestAppContext) {
    let data = Mutable::new(row(""));
    let (entity, vcx, services) = mount_editor(cx, &data);
    services
        .refuse_promotions
        .store(true, std::sync::atomic::Ordering::SeqCst);

    type_text(vcx, &entity, "TODO milk");

    assert_eq!(
        visible(vcx, &entity),
        "TODO milk",
        "REGRESSION: the engine refused and stored the typed text verbatim, so \
         the editor must show it again — a stripped buffer here is the data-loss bug"
    );
    assert_eq!(
        caret(vcx, &entity),
        9,
        "the caret follows the restored text, not the stripped one"
    );
    assert_eq!(
        services
            .recorded_intents()
            .iter()
            .filter(|i| i.op_name == "promote_task_keyword")
            .count(),
        1,
        "one proposal, one verdict — the refusal is not retried"
    );
}

/// The refusal restores through the CELL arm too: the recovery lives in the
/// adapter, not in the SqlOnly-only code path.
#[gpui::test]
fn a_refused_promotion_puts_the_keyword_back_with_a_cell(cx: &mut TestAppContext) {
    let data = Mutable::new(row(""));
    let (entity, vcx, services) = mount_cell_editor(cx, &data);
    services
        .refuse_promotions
        .store(true, std::sync::atomic::Ordering::SeqCst);

    type_text(vcx, &entity, "TODO milk");

    assert_eq!(visible(vcx, &entity), "TODO milk");
}
