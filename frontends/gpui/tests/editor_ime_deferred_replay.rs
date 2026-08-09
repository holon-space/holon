//! Windowed rung for the IME deferred-replay mid-focus clobber class
//! (EditorBufferOwnership Amendment 3,
//! `docs/Plans/EditorBufferOwnership-2026-07-20.md`).
//!
//! THE BUG CLASS (desktop GPUI, real keyboard / IME): the user is mid-IME
//! composition (a German umlaut dead-key sequence leaves `ime_marked_range()`
//! `Some`) when an EXTERNAL write lands on this row — a peer edit, a file
//! reload, or a CDC echo delivered through the per-row data `Mutable`. Without
//! Amendment 3 the data-sync convergence would `set_value` the visible
//! `InputState` synchronously and CLOBBER the in-flight composed text (the
//! recurring dogfood "duplicate on Enter" / stale-buffer-clobber reports trace
//! to this interleave of marked-text replay with focus changes). Amendment 3's
//! contract: a converge that arrives mid-composition is DEFERRED on the view
//! model (`set_pending_directive`), never applied synchronously, and REPLAYED
//! on the composition-end / focus-transition edge (`replay_pending_directive`)
//! — never silently dropped (which would leave the buffer stale until an
//! unrelated later echo).
//!
//! These tests drive the REAL adapter path — `EditorView::_data_subscription`
//! → `EditorViewModel::converge_from_data_sync` →
//! `EditorView::converge_or_defer` → `replay_pending_directive` — through a
//! real GPUI window and a real gpui_component `InputState`. The IME composition
//! is a real `EntityInputHandler::replace_and_mark_text_in_range` on the input
//! entity (the same platform entry point macOS/mobile IME uses), so
//! `ime_marked_range()` is `Some` exactly as in production. The external write
//! is pushed through the same per-row data `Mutable` the production
//! `ReactiveRowSet` feeds the subscription with (the fallback the
//! trailing-space rung `editor_trailing_space_echo.rs` established — a full CDC
//! round-trip needs a live SQL backend). The FIRST push (content `"cafe"`, seq
//! 100) is adopted via the real `Converge` arm, reconstructing the post-typing
//! state through production code without simulating keystrokes.
//!
//! RED-FOR-THE-RIGHT-REASON (Amendment 2 throwaway-rev methodology, as the plan
//! prescribes for retroactive coverage of a landed amendment): each assertion
//! was proven to bite by a jj-local throwaway edit that reverts the amendment,
//! then abandoned. See the reginject logs referenced in the PR. Concretely:
//!   * neutering the `ime_marked_range().is_some()` guard in
//!     `converge_or_defer` (converge immediately, never defer) makes the
//!     composed text get clobbered →
//!     `defers_external_converge_and_preserves_composed_text` RED.
//!   * early-returning `replay_pending_directive` (deferred directive silently
//!     dropped) makes the buffer stay stale after the composition-end edge →
//!     `replays_on_composition_end_after_deferred_converge` RED.
//!
//! Run: cargo test -p holon-gpui --test editor_ime_deferred_replay

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

/// Build a per-row `DataRow` the way the projection feeds the editor's
/// data-sync subscription: `content` plus (optionally) the `write_seq` ordering
/// token.
fn row(content: &str, write_seq: Option<i64>) -> Arc<DataRow> {
    let mut m = DataRow::new();
    m.insert("content".to_string(), Value::String(content.to_string()));
    if let Some(seq) = write_seq {
        m.insert("write_seq".to_string(), Value::Integer(seq));
    }
    Arc::new(m)
}

/// Construct a real no-cell (SqlOnly) `EditorView` bound to `data` and return
/// its window entity + the windowed test context (kept alive so the test can
/// focus/blur the input and drive real IME marked-text events). `TestServices`
/// exposes no `Cell<String>`, so the editor takes the SqlOnly data-sync path —
/// the exact production path Amendment 3's deferral lives on.
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
    let row_id_for_view = "block:ime-deferred-replay-test".to_string();

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

/// The visible `InputState` value (includes any in-flight IME marked text).
fn visible(vcx: &mut VisualTestContext, entity: &gpui::Entity<EditorView>) -> String {
    entity.read_with(vcx, |ev, cx| ev.input_entity().read(cx).value().to_string())
}

/// Whether an IME composition is in progress on the visible input.
fn is_composing(vcx: &mut VisualTestContext, entity: &gpui::Entity<EditorView>) -> bool {
    entity.read_with(vcx, |ev, cx| {
        ev.input_entity().read(cx).ime_marked_range().is_some()
    })
}

/// Seed the post-typing state through the real `Converge` arm: content at
/// `seq` becomes the visible buffer AND `last_local_seq`, mirroring a settled
/// editor whose SqlOnly content is `content`.
fn seed(
    vcx: &mut VisualTestContext,
    entity: &gpui::Entity<EditorView>,
    data: &Mutable<Arc<DataRow>>,
    content: &str,
    seq: i64,
) {
    data.set(row(content, Some(seq)));
    vcx.run_until_parked();
    assert_eq!(
        visible(vcx, entity),
        content,
        "precondition: seed write should have converged the buffer"
    );
}

/// Begin an IME composition: insert `marked` as marked text at the end of the
/// current value, leaving `ime_marked_range()` `Some` (the real dead-key /
/// umlaut composition state).
fn begin_composition(vcx: &mut VisualTestContext, entity: &gpui::Entity<EditorView>, marked: &str) {
    let input = entity.read_with(vcx, |ev, _| ev.input_entity().clone());
    let end = input.read_with(vcx, |s, _| s.value().chars().count());
    let sel = marked.chars().count();
    input.update_in(vcx, |state, window, cx| {
        state.replace_and_mark_text_in_range(Some(end..end), marked, Some(0..sel), window, cx);
    });
    vcx.run_until_parked();
}

/// End the IME composition through the platform commit entry point
/// (`replace_text_in_range`): it replaces the marked range, clears
/// `ime_marked_range()`, and emits `InputEvent::Change` — the composition-end
/// edge Amendment 3 replays a deferred directive on (the `InputEvent::Change`
/// handler in `editor_view.rs`). Passing empty text discards the composed
/// character (Escape-cancel), returning the value to its pre-composition
/// baseline so no superseding local write is stamped (`apply_local_edit`
/// sees `new_text == buffer` and returns `None`).
fn end_composition(vcx: &mut VisualTestContext, entity: &gpui::Entity<EditorView>) {
    let input = entity.read_with(vcx, |ev, _| ev.input_entity().clone());
    input.update_in(vcx, |state, window, cx| {
        state.replace_text_in_range(None, "", window, cx);
    });
    vcx.run_until_parked();
}

/// PRIMARY RUNG — the clobber. While an IME composition is in progress, an
/// external converge (a peer edit at a higher `write_seq`) must be DEFERRED,
/// not applied: the visible composed text is preserved. Pre-Amendment-3 the
/// data-sync path `set_value`s synchronously and the composed umlaut is gone.
#[gpui::test]
fn defers_external_converge_and_preserves_composed_text(cx: &mut TestAppContext) {
    let data = Mutable::new(row("", None));
    let (entity, vcx, _services) = mount_editor(cx, &data);

    seed(vcx, &entity, &data, "cafe", 100);

    // Start composing an umlaut at the end → visible "cafeü", marked range Some.
    begin_composition(vcx, &entity, "ü");
    assert_eq!(visible(vcx, &entity), "cafeü", "composition in progress");
    assert!(is_composing(vcx, &entity), "IME marked range must be Some");

    // An external peer edit lands mid-composition at a strictly newer seq.
    data.set(row("peer edit", Some(200)));
    vcx.run_until_parked();

    assert!(
        is_composing(vcx, &entity),
        "the mid-composition converge must not have ended the IME composition"
    );
    assert_eq!(
        visible(vcx, &entity),
        "cafeü",
        "CLOBBER REGRESSION: an external converge that arrived while \
         ime_marked_range() was Some overwrote the in-flight composed text. \
         converge_or_defer must set_pending_directive (defer), never set_value \
         synchronously, while a composition is in progress (Amendment 3)."
    );
}

/// REPLAY RUNG — the deferred directive must not be silently dropped. An
/// external converge is deferred mid-composition; when the composition ends on
/// the `InputEvent::Change` edge (`ime_marked_range()` cleared) Amendment 3
/// replays the deferred directive and the buffer converges to the external
/// authority. Pre-Amendment-3 (no replay) the buffer stays stale at its
/// pre-composition form until an unrelated later echo.
#[gpui::test]
fn replays_on_composition_end_after_deferred_converge(cx: &mut TestAppContext) {
    let data = Mutable::new(row("", None));
    let (entity, vcx, _services) = mount_editor(cx, &data);

    seed(vcx, &entity, &data, "cafe", 100);

    // Compose an umlaut, then let an external peer edit arrive mid-composition
    // — deferred, the composed buffer preserved.
    begin_composition(vcx, &entity, "ü");
    data.set(row("peer edit", Some(200)));
    vcx.run_until_parked();
    assert_eq!(
        visible(vcx, &entity),
        "cafeü",
        "precondition: the external converge is deferred during composition"
    );

    // Composition ends (Escape-cancel: the composed char is discarded, no
    // superseding local write) → the `InputEvent::Change` edge replays the
    // deferred directive.
    end_composition(vcx, &entity);
    assert!(!is_composing(vcx, &entity), "composition ended");

    assert_eq!(
        visible(vcx, &entity),
        "peer edit",
        "SILENT-DROP REGRESSION: the directive deferred during the composition \
         was not replayed on the composition-end edge, so the buffer stayed \
         stale at its pre-composition form. replay_pending_directive must \
         converge the visible InputState to the pending external authority \
         when the marked range clears (Amendment 3)."
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
