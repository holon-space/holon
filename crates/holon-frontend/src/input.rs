use std::collections::BTreeSet;

use holon_api::render_types::OperationDescriptor;

use crate::navigation::{CursorHint, CursorPlacement, NavDirection};

/// A key on the keyboard. Modifiers and regular keys in one enum
/// so that chords are just `BTreeSet<Key>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Key {
    // Modifiers
    Cmd,
    Ctrl,
    Alt,
    Shift,

    // Navigation
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,

    // Editing
    Enter,
    Backspace,
    Delete,
    Escape,
    Space,

    // Letters (uppercase for canonical form)
    Char(char),

    // Function keys
    F(u8),
}

/// Input event that a widget can emit upward when it doesn't handle it.
#[derive(Debug, Clone)]
pub enum WidgetInput {
    /// Semantic: an editable text detected cursor at its boundary and wants
    /// to move to the next/previous block.
    Navigate {
        direction: NavDirection,
        hint: CursorHint,
    },

    /// A set of keys pressed simultaneously. Order doesn't matter —
    /// `{Cmd, Enter}` == `{Enter, Cmd}`.
    ///
    /// For chord keyboards that press multiple letter keys at once,
    /// `{S, A, V, E}` is a valid chord distinct from typing s-a-v-e sequentially.
    KeyChord { keys: BTreeSet<Key> },
}

impl WidgetInput {
    /// Convenience: build a KeyChord from a slice of keys.
    pub fn chord(keys: &[Key]) -> Self {
        Self::KeyChord {
            keys: keys.iter().cloned().collect(),
        }
    }
}

/// What happened when a widget tried to handle an input.
#[derive(Debug, Clone)]
pub enum InputAction {
    /// Input was consumed. Stop bubbling.
    Handled,

    /// Execute an operation on an entity (e.g., cycle_task_state on a block).
    /// The frontend dispatches this via `FrontendSession::execute_operation`.
    ExecuteOperation {
        entity_name: String,
        operation: OperationDescriptor,
        entity_id: String,
    },

    /// Focus a different block (navigation result).
    Focus {
        block_id: String,
        placement: CursorPlacement,
    },
}

/// Result of a handler trying to process an input.
pub type HandleResult = Option<InputAction>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_order_independent() {
        let a = WidgetInput::chord(&[Key::Cmd, Key::Enter]);
        let b = WidgetInput::chord(&[Key::Enter, Key::Cmd]);
        match (&a, &b) {
            (WidgetInput::KeyChord { keys: ka }, WidgetInput::KeyChord { keys: kb }) => {
                assert_eq!(ka, kb);
            }
            _ => panic!("expected KeyChord"),
        }
    }

    #[test]
    fn multi_char_chord() {
        let chord = WidgetInput::chord(&[
            Key::Char('s'),
            Key::Char('a'),
            Key::Char('v'),
            Key::Char('e'),
        ]);
        match chord {
            WidgetInput::KeyChord { keys } => {
                assert_eq!(keys.len(), 4);
                assert!(keys.contains(&Key::Char('a')));
                assert!(keys.contains(&Key::Char('e')));
            }
            _ => panic!("expected KeyChord"),
        }
    }
}
