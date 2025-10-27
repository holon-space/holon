use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A key on the keyboard. Modifiers and regular keys in one enum
/// so that chords are just `BTreeSet<Key>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// A set of keys pressed simultaneously. Order doesn't matter —
/// `KeyChord([Cmd, Enter])` == `KeyChord([Enter, Cmd])`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct KeyChord(pub BTreeSet<Key>);

impl KeyChord {
    pub fn new(keys: &[Key]) -> Self {
        Self(keys.iter().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_order_independent() {
        let a = KeyChord::new(&[Key::Cmd, Key::Enter]);
        let b = KeyChord::new(&[Key::Enter, Key::Cmd]);
        assert_eq!(a, b);
    }

    #[test]
    fn serde_round_trip() {
        let chord = KeyChord::new(&[Key::Cmd, Key::Enter]);
        let json = serde_json::to_string(&chord).unwrap();
        let deserialized: KeyChord = serde_json::from_str(&json).unwrap();
        assert_eq!(chord, deserialized);
    }

    #[test]
    fn char_key_serde() {
        let chord = KeyChord::new(&[Key::Ctrl, Key::Char('s')]);
        let json = serde_json::to_string(&chord).unwrap();
        let deserialized: KeyChord = serde_json::from_str(&json).unwrap();
        assert_eq!(chord, deserialized);
    }
}
