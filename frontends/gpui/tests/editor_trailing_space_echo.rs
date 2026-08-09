//! Windowed rung for the trailing-space echo-suppression fix.
//!
//! THE BUG (desktop GPUI, SqlOnly / no-cell blocks): the user types a trailing
//! space; ~100ms later it vanishes. Chain: the editor's `InputEvent::Change`
//! stamps `write_seq = N` and dispatches `set_field content = "foo "`;
//! `SqlOperationProvider::trimmed_content` trims the trailing whitespace and
//! stores `"foo"` WITH the SAME `write_seq = N` (the trim does not re-stamp);
//! the CDC echo `("foo", seq = N)` reaches the editor's `_data_subscription`
//! while `last_local_seq == N` and the focused buffer is still `"foo "`. The
//! old `>=` convergence rule could not tell that SQL-canonicalized echo of the
//! user's OWN in-flight write apart from a genuinely newer external authority,
//! so it converged and ate the space.
//!
//! These tests drive the REAL `EditorView::_data_subscription` /
//! `evaluate_data_sync_echo` / `converge_input` path through a real GPUI
//! window. A full CDC round-trip needs a live SQL backend, so the rung instead
//! pushes synthetic rows through the same per-row data `Mutable` the production
//! `ReactiveRowSet` feeds the subscription with — the fallback the fix's task
//! explicitly sanctions. The FIRST push (content `"foo "`, seq N) is adopted
//! via the real `Converge` arm, which sets `last_local_seq = N` AND the visible
//! buffer to `"foo "` — i.e. it reconstructs the exact post-typing state
//! through production code, without needing to simulate keystrokes or poke
//! private fields. The SECOND push (`"foo"`, seq N) is the SQL-canonicalized
//! echo whose handling is under test.
//!
//! Run: cargo test -p holon-gpui --test editor_trailing_space_echo

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::TestAppContext;
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
/// token. Absent `write_seq` mirrors a pre-typing / non-editor write.
fn row(content: &str, write_seq: Option<i64>) -> Arc<DataRow> {
    let mut m = DataRow::new();
    m.insert("content".to_string(), Value::String(content.to_string()));
    if let Some(seq) = write_seq {
        m.insert("write_seq".to_string(), Value::Integer(seq));
    }
    Arc::new(m)
}

/// Construct a real no-cell (SqlOnly) `EditorView` bound to `data`, and return
/// its window entity + visual context. `TestServices::editable_text` errors, so
/// no `Cell<String>` attaches and the editor takes the SqlOnly data-sync path.
fn mount_editor(
    cx: &mut TestAppContext,
    data: &Mutable<Arc<DataRow>>,
) -> (gpui::Entity<EditorView>, String) {
    cx.update(|cx| gpui_component::init(cx));
    let services: Arc<dyn BuilderServices> = TestServices::new();
    let data_handle = data.read_only();
    let row_id = "block:trailing-space-test".to_string();
    let row_id_for_view = row_id.clone();

    // The window's first layer MUST be a `gpui_component::Root` (its render path
    // reads window-level Root state), so wrap the real `EditorView` in a Root and
    // smuggle the editor entity back out for assertions.
    let slot: Rc<RefCell<Option<gpui::Entity<EditorView>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let (_root, vcx) = cx.add_window_view(move |window, cx| {
        let editor = cx.new(|cx| {
            EditorView::new(
                "editor-el".to_string(),
                String::new(), // initial content — matches the initial data row below
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
    (entity, row_id)
}

fn buffer(cx: &mut TestAppContext, entity: &gpui::Entity<EditorView>) -> String {
    entity.read_with(cx, |ev, cx| ev.input_entity().read(cx).value().to_string())
}

/// PRIMARY RUNG. Type a trailing space, then deliver the SQL-canonicalized echo
/// at the SAME write-seq. Pre-fix (`>=` rule) this Converges and the space is
/// gone; post-fix the same-seq trailing-whitespace echo is adopted as baseline
/// and the focused buffer keeps its space.
#[gpui::test]
fn trailing_space_survives_same_seq_canonicalized_echo(cx: &mut TestAppContext) {
    let data = Mutable::new(row("", None));
    let (entity, _row_id) = mount_editor(cx, &data);

    // Reconstruct the post-typing state through the real Converge arm:
    // content "foo " at seq 5000 → buffer "foo ", last_local_seq := 5000.
    data.set(row("foo ", Some(5000)));
    cx.run_until_parked();
    assert_eq!(
        buffer(cx, &entity),
        "foo ",
        "precondition: the seed write should have converged the buffer to the typed 'foo '"
    );

    // The SQL-canonicalized echo of that OWN write: "foo" (trailing space
    // trimmed on store) carrying the SAME seq 5000.
    data.set(row("foo", Some(5000)));
    cx.run_until_parked();

    assert_eq!(
        buffer(cx, &entity),
        "foo ",
        "REGRESSION: the same-seq canonicalized echo of the editor's own write \
         ate the trailing space from the focused buffer. evaluate_data_sync_echo \
         must return AdoptBaseline (not Converge) when echo_seq == last_local_seq \
         AND new_value == current.trim_end()."
    );
}

/// CLASS EXTENSION RUNG. The server also trims the FIRST line's trailing
/// whitespace in multiline text content, so a space typed at the end of a
/// multiline block's first line is the SAME bug class. Pre-fix (a `trim_end`
/// discriminator, which can't see the interior stripped space) this Converges
/// and the space is gone; post-fix the full server canonicalization recognizes
/// the echo and keeps the buffer.
#[gpui::test]
fn trailing_space_on_first_line_of_multiline_survives_same_seq_echo(cx: &mut TestAppContext) {
    let data = Mutable::new(row("", None));
    let (entity, _row_id) = mount_editor(cx, &data);

    // Reconstruct the post-typing state: first line "foo " (trailing space),
    // second line "bar", at seq 8000 → buffer + last_local_seq via Converge.
    data.set(row("foo \nbar", Some(8000)));
    cx.run_until_parked();
    assert_eq!(
        buffer(cx, &entity),
        "foo \nbar",
        "precondition: the seed write should have converged the multiline buffer"
    );

    // The SQL-canonicalized echo: the first-line trailing space is trimmed
    // ("foo\nbar"), carrying the SAME seq 8000.
    data.set(row("foo\nbar", Some(8000)));
    cx.run_until_parked();

    assert_eq!(
        buffer(cx, &entity),
        "foo \nbar",
        "REGRESSION: the same-seq echo canonicalized the first line's trailing \
         space away and the discriminator failed to recognize it as the editor's \
         own write — evaluate_data_sync_echo must compare against the FULL server \
         canonicalization (holon_api::content_canonical), not just current.trim_end()."
    );
}

/// GUARD ON THE FORK RESOLUTION. Non-editor writers (split/join/org) do NOT
/// bump `write_seq`, so a genuine structural truncation ALSO echoes at
/// `seq == last_local`. That must still converge — it is a substantive change,
/// not a trailing-whitespace trim of the buffer. This proves the fix
/// discriminates by content shape and does not over-suppress real writes.
#[gpui::test]
fn same_seq_substantive_truncation_still_converges(cx: &mut TestAppContext) {
    let data = Mutable::new(row("", None));
    let (entity, _row_id) = mount_editor(cx, &data);

    data.set(row("hello world", Some(7000)));
    cx.run_until_parked();
    assert_eq!(buffer(cx, &entity), "hello world", "precondition seed");

    // A split truncation to "hello" at the editor's unchanged seq. "hello" is
    // NOT "hello world".trim_end(), so it is a real external change → converge.
    data.set(row("hello", Some(7000)));
    cx.run_until_parked();

    assert_eq!(
        buffer(cx, &entity),
        "hello",
        "same-seq SUBSTANTIVE change (a real structural write that didn't bump \
         write_seq) must still converge — the trailing-space fix must not swallow it"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
