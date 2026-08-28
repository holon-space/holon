//! Windowed rung for the Android soft-keyboard commit SEQUENCE
//! (bugfunnel defect 7: typing `H`,`i`,`a`,`s` landed `saiH` in the vault).
//!
//! THE BUG CLASS: on Android every `commitText` from the IME arrives as
//! `nativeReplaceText(start, end, text)` where `start..end` is a range the IME
//! computed against ITS OWN mirror of the editor. The fork re-seeds that mirror
//! from GPUI once per frame (`android::text_input::sync_state_to_java` →
//! `GpuiTextInputView.updateEditingState` → `applyEditingState`), reading the
//! caret back out of `EntityInputHandler::selected_text_range`. So the caret
//! the NEXT commit targets is the caret this editor reports after the PREVIOUS
//! one. If that reported caret does not advance past the inserted text, every
//! commit targets the same offset and the typed string comes out reversed.
//!
//! These rungs drive the real production entry point on a real windowed
//! `EditorView` + gpui_component `InputState`: each character is committed at
//! the range the editor itself reports through `selected_text_range`, never at
//! a range the test computed. That is the exact loop-back Android runs; nothing
//! about the Java mirror is modelled, because the mirror is a pure function of
//! what these two trait methods return.
//!
//! ENVIRONMENT GAP (device-only): the JNI hop
//! (`Java_dev_gpui_mobile_GpuiTextInputView_nativeReplaceText` → command queue
//! → `drain_into`) and the once-per-frame `set_input_handler` that triggers
//! `sync_state_to_java` live in the `gpui-mobile` fork and have no headless
//! harness. This rung covers the editor half of the contract — that the caret
//! reported back after a commit is the one the next commit must use.
//!
//! Run: cargo test -p holon-gpui --test editor_ime_commit_sequence

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

fn row(content: &str, write_seq: Option<i64>) -> Arc<DataRow> {
    let mut m = DataRow::new();
    m.insert("content".to_string(), Value::String(content.to_string()));
    if let Some(seq) = write_seq {
        m.insert("write_seq".to_string(), Value::Integer(seq));
    }
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
    let row_id_for_view = "block:ime-commit-sequence-test".to_string();

    let slot: Rc<RefCell<Option<gpui::Entity<EditorView>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let (_root, vcx) = cx.add_window_view(move |window, cx| {
        let editor = cx.new(|cx| {
            EditorView::new(
                "editor-el".to_string(),
                String::new(),
                "content".to_string(),
                row_id_for_view,
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

fn visible(vcx: &mut VisualTestContext, entity: &gpui::Entity<EditorView>) -> String {
    entity.read_with(vcx, |ev, cx| ev.input_entity().read(cx).value().to_string())
}

/// The caret/selection this editor reports to the platform, in UTF-16 code
/// units — what `sync_state_to_java` ships to the IME mirror each frame.
fn reported_selection(
    vcx: &mut VisualTestContext,
    entity: &gpui::Entity<EditorView>,
) -> std::ops::Range<usize> {
    let input = entity.read_with(vcx, |ev, _| ev.input_entity().clone());
    input
        .update_in(vcx, |state, window, cx| {
            state.selected_text_range(true, window, cx)
        })
        .expect("the input handler always reports a selection")
        .range
}

/// One soft-keyboard `commitText`, exactly as the Android path performs it:
/// the replacement range is the selection the editor reported after the
/// PREVIOUS commit, never a range the caller tracked itself.
fn commit_at_reported_caret(
    vcx: &mut VisualTestContext,
    entity: &gpui::Entity<EditorView>,
    text: &str,
) {
    let range = reported_selection(vcx, entity);
    let input = entity.read_with(vcx, |ev, _| ev.input_entity().clone());
    input.update_in(vcx, |state, window, cx| {
        state.replace_text_in_range(Some(range), text, window, cx);
    });
    vcx.run_until_parked();
}

/// SAME-FRAME RUNG — the caret must be correct the INSTANT the commit returns,
/// with no deferred work allowed to run in between. Android reads it that way:
/// `drain_into` applies the queued commit in the frame callback and
/// `sync_state_to_java` reads the caret back at the end of that SAME draw, so a
/// caret that is only correct once tasks have settled would still ship a stale
/// offset to the IME mirror. The other rungs park between commits and would not
/// see such a lag.
#[gpui::test]
fn reported_caret_is_correct_within_the_committing_frame(cx: &mut TestAppContext) {
    let data = Mutable::new(row("", None));
    let (entity, vcx, _services) = mount_editor(cx, &data);

    let input = entity.read_with(vcx, |ev, _| ev.input_entity().clone());
    for (ch, expected) in [("H", 1), ("i", 2), ("a", 3), ("s", 4)] {
        let range = input
            .update_in(vcx, |state, window, cx| {
                state.selected_text_range(true, window, cx)
            })
            .expect("the input handler always reports a selection")
            .range;
        // Commit and read the caret back inside ONE update — nothing parks, so
        // only a synchronously-correct caret can satisfy this.
        let after = input.update_in(vcx, |state, window, cx| {
            state.replace_text_in_range(Some(range), ch, window, cx);
            state
                .selected_text_range(true, window, cx)
                .expect("the input handler always reports a selection")
                .range
        });
        assert_eq!(
            after,
            expected..expected,
            "the caret must already be past the committed text when the commit \
             returns — Android ships it to the IME mirror later in the same \
             frame, before any deferred work runs"
        );
    }

    vcx.run_until_parked();
    assert_eq!(visible(vcx, &entity), "Hias");
}

/// PRIMARY RUNG — typing four characters through the soft-keyboard commit loop
/// must spell them forwards. Pre-fix on Android each commit landed at offset 0
/// and `Hias` arrived as `saiH`.
#[gpui::test]
fn soft_keyboard_commit_sequence_types_forwards(cx: &mut TestAppContext) {
    let data = Mutable::new(row("", None));
    let (entity, vcx, _services) = mount_editor(cx, &data);

    for ch in ["H", "i", "a", "s"] {
        commit_at_reported_caret(vcx, &entity, ch);
    }

    assert_eq!(
        visible(vcx, &entity),
        "Hias",
        "REVERSED-TYPING REGRESSION: committing each character at the selection \
         the editor reported after the previous commit produced the characters \
         in reverse order. `selected_text_range` must report the caret AFTER \
         the inserted text — it is the range the platform IME mirror targets \
         with the next `commitText`."
    );
}

/// CARET RUNG — the reported caret must sit after the inserted text, not at
/// its start. This is the invariant the sequence rung above rests on, asserted
/// directly so a failure localises to the reporting side.
#[gpui::test]
fn reported_caret_advances_past_each_commit(cx: &mut TestAppContext) {
    let data = Mutable::new(row("", None));
    let (entity, vcx, _services) = mount_editor(cx, &data);

    commit_at_reported_caret(vcx, &entity, "Hi");
    assert_eq!(
        reported_selection(vcx, &entity),
        2..2,
        "the caret reported to the platform must sit after the committed text"
    );

    commit_at_reported_caret(vcx, &entity, "as");
    assert_eq!(
        reported_selection(vcx, &entity),
        4..4,
        "the caret reported to the platform must keep advancing across commits"
    );
    assert_eq!(visible(vcx, &entity), "Hias");
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
