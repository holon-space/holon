//! Editor-layer fast-UI tests.
//!
//! Exercises `EditorViewModel` end-to-end with a real `LinkProvider`
//! running against canned `popup_query` results via `TestServices`. No
//! GPUI views, no SQL backend — just the controller, popup menu, and
//! provider pipeline, so regressions in the refactored `BuilderServices`
//! plumbing surface here before they ever reach a running app.
//!
//! See the hand-off notes next to the Phase 2 refactor for context: the
//! doc-link autocomplete was the narrowly-missed regression vector when
//! `LinkProvider` switched from `Arc<FrontendSession>` to
//! `Arc<dyn BuilderServices>`, so these tests lock in the round-trip.

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::render_types::OperationWiring;
use holon_api::Value;
use holon_frontend::editor_view_model::{EditorAction, EditorKey, EditorViewModel};
use holon_frontend::input_trigger::InputTrigger;
use holon_frontend::reactive::BuilderServices;

use support::TestServices;

/// Build a LinkCandidate with the shape LinkProvider expects.
fn row(id: &str, label: &str) -> holon_api::LinkCandidate {
    holon_api::LinkCandidate {
        id: holon_api::EntityUri::parse(id).expect("test fixture id must be a valid EntityUri"),
        label: label.to_string(),
    }
}

fn doc_link_controller() -> EditorViewModel {
    // Minimal operation wiring — LinkProvider doesn't care about operations,
    // but EditorViewModel::new expects a non-empty-ish list when a
    // `set_field` path exists. We don't exercise that path here, so an
    // empty vec is fine.
    let ops: Vec<OperationWiring> = Vec::new();
    let triggers = vec![InputTrigger::TextPrefix {
        prefix: "[[".to_string(),
        action: "doc_link".to_string(),
        at_line_start: false,
    }];
    let context = HashMap::from([("id".into(), Value::String("block-1".into()))]);
    EditorViewModel::new(ops, triggers, context, "content".into(), String::new())
}

/// Full round-trip: type `[[Proj`, popup activates backed by `LinkProvider`,
/// the signal pipeline emits canned rows through `popup_query`, selecting
/// the first item with `Enter` returns `InsertText` with the resolved
/// `[[id][label]]` link.
///
/// Guards the architectural contract of Phase 2: `LinkProvider` depends on
/// `Arc<dyn BuilderServices>`, not `Arc<FrontendSession>`, and the whole
/// path is testable without a GPUI view, a real tokio-backed query, or a
/// backend. Any regression in the plumbing (services dropped between
/// `EditorViewModel` and `LinkProvider`, popup activation skipping
/// provider construction, `on_select` format drift) fails this test.
#[test]
fn doc_link_round_trip_emits_resolved_insert() {
    let services = TestServices::with_popup_results(vec![
        row("block:proj-alpha", "Project Alpha"),
        row("block:proj-beta", "Project Beta"),
    ]);
    let handle = services.runtime_handle();

    let mut ctrl = doc_link_controller();
    ctrl.set_async_context(services.clone() as Arc<dyn BuilderServices>);

    // Type `[[Proj` → doc_link trigger fires, popup activates, signal is
    // returned. The signal wraps `LinkProvider::candidates` which spawns
    // a `popup_query` future on the runtime from `services`.
    let action = ctrl.on_text_changed("see [[Proj", 10);
    let signal = match action {
        EditorAction::PopupActivated { signal } => signal,
        other => panic!("expected PopupActivated, got {other:?}"),
    };
    assert!(
        ctrl.is_popup_active(),
        "popup should be active after activation"
    );

    // Drive the signal to its first non-empty emission. `map_future` emits
    // `None` while the spawned query is pending and `Some(items)` once it
    // resolves; our `.map(unwrap_or_default)` in `LinkProvider` collapses
    // that to `Vec<PopupItem>`, so the first tick is empty and the second
    // carries the canned rows. Entering the tokio handle lets the spawned
    // task make progress while `futures::executor::block_on` drives the
    // outer stream.
    let items = {
        use futures::StreamExt;
        use futures_signals::signal::SignalExt;
        let _guard = handle.enter();
        futures::executor::block_on(async move {
            let mut stream = Box::pin(signal.to_stream());
            for _ in 0..20 {
                if let Some(items) = stream.next().await {
                    if !items.is_empty() {
                        return items;
                    }
                }
            }
            panic!("signal never produced non-empty items after 20 ticks");
        })
    };

    // The signal closure in `PopupMenu::activate` writes every emission
    // into `popup.items`, so pumping the signal is what lets
    // `on_key(Enter)` find a selected item to forward to
    // `LinkProvider::on_select`.
    assert!(
        items.iter().any(|i| i.id == "block:proj-alpha"),
        "canned row `block:proj-alpha` not in popup items: {items:?}"
    );
    assert!(
        items.iter().any(|i| i.id.starts_with("__create_new__")),
        "LinkProvider should append a 'Create new' entry: {items:?}"
    );

    // Enter selects the first canned row (selected_index starts at 0, which
    // is the first real result since items come from the DB query first and
    // 'Create new' is appended last).
    match ctrl.on_key(EditorKey::Enter) {
        EditorAction::InsertText {
            replacement,
            prefix_start,
        } => {
            assert_eq!(replacement, "[[block:proj-alpha][Project Alpha]]");
            // `prefix_start` is the column where `[[` began in the line;
            // `on_text_changed("see [[Proj", 10)` puts `[[` at column 4.
            assert_eq!(prefix_start, 4);
        }
        other => panic!("expected InsertText, got {other:?}"),
    }

    assert!(
        !ctrl.is_popup_active(),
        "popup should dismiss after a selection"
    );
}
