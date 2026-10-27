//! The window-level keymap must be readable by an agent, not only by GPUI.
//!
//! `list_keybindings` reports what the frontend publishes; if a chord is
//! registered with `cx.bind_keys` but never published, an agent obeying "read
//! a shortcut before you send it" concludes it is unbound (dogfood
//! 2026-08-07). These tests pin the two properties that make the registry
//! trustworthy: undo/redo are IN it, and every chord in it is expressible in
//! the wire vocabulary `send_key_chord` accepts.

use holon_gpui::window_key_bindings;

#[test]
fn undo_and_redo_are_in_the_published_window_registry() {
    let rows = window_key_bindings();
    let undo = rows
        .iter()
        .find(|r| r.action == "undo")
        .expect("undo must be discoverable through the window registry");
    let redo = rows
        .iter()
        .find(|r| r.action == "redo")
        .expect("redo must be discoverable through the window registry");

    #[cfg(target_os = "macos")]
    {
        assert_eq!(undo.chord, "cmd-z");
        assert_eq!(redo.chord, "cmd-shift-z");
    }
    #[cfg(not(target_os = "macos"))]
    {
        assert_eq!(undo.chord, "ctrl-z");
        assert_eq!(redo.chord, "ctrl-y");
    }
}

#[test]
fn every_window_chord_is_expressible_in_the_send_key_chord_vocabulary() {
    for row in window_key_bindings() {
        for segment in row.chord.split('-') {
            segment.parse::<holon_api::Key>().unwrap_or_else(|e| {
                panic!(
                    "action {:?}: chord {:?} segment {segment:?} cannot be parsed back ({e}), so \
                     an agent could not send it",
                    row.action, row.chord
                )
            });
        }
    }
}

#[test]
fn a_context_scoped_chord_declares_its_context() {
    let rows = window_key_bindings();
    let tip = rows
        .iter()
        .find(|r| r.action == "turn_into_page")
        .expect("turn_into_page is registered");
    assert_eq!(
        tip.context,
        Some("Input"),
        "an editor-only chord must say so, else an agent reads it as always active"
    );
    let undo = rows
        .iter()
        .find(|r| r.action == "undo")
        .expect("registered");
    assert_eq!(undo.context, None);
}

#[test]
fn no_two_window_actions_claim_the_same_chord() {
    let rows = window_key_bindings();
    let mut seen: std::collections::HashMap<(&str, Option<&str>), &str> =
        std::collections::HashMap::new();
    for row in &rows {
        let key = (row.chord.as_str(), row.context);
        if let Some(prev) = seen.insert(key, row.action) {
            panic!(
                "chord {:?} (context {:?}) is claimed by both {prev:?} and {:?} — one of them can \
                 never fire, and the registry would report a lie",
                row.chord, row.context, row.action
            );
        }
    }
}
