use std::collections::BTreeSet;

use serde::Deserialize;
use serde::Serialize;

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

/// The canonical wire name of a key — what [`FromStr`](std::str::FromStr)
/// reads back and what the MCP key tools speak.
impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Key::Cmd => f.write_str("cmd"),
            Key::Ctrl => f.write_str("ctrl"),
            Key::Alt => f.write_str("alt"),
            Key::Shift => f.write_str("shift"),
            Key::Up => f.write_str("up"),
            Key::Down => f.write_str("down"),
            Key::Left => f.write_str("left"),
            Key::Right => f.write_str("right"),
            Key::Home => f.write_str("home"),
            Key::End => f.write_str("end"),
            Key::PageUp => f.write_str("pageup"),
            Key::PageDown => f.write_str("pagedown"),
            Key::Tab => f.write_str("tab"),
            Key::Enter => f.write_str("enter"),
            Key::Backspace => f.write_str("backspace"),
            Key::Delete => f.write_str("delete"),
            Key::Escape => f.write_str("escape"),
            Key::Space => f.write_str("space"),
            Key::Char(c) => write!(f, "{c}"),
            Key::F(n) => write!(f, "f{n}"),
        }
    }
}

/// Case-insensitive, and accepts the common platform aliases alongside each
/// canonical name.
impl std::str::FromStr for Key {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "cmd" | "command" | "platform" => Ok(Key::Cmd),
            "ctrl" | "control" => Ok(Key::Ctrl),
            "alt" | "option" => Ok(Key::Alt),
            "shift" => Ok(Key::Shift),
            "up" => Ok(Key::Up),
            "down" => Ok(Key::Down),
            "left" => Ok(Key::Left),
            "right" => Ok(Key::Right),
            "home" => Ok(Key::Home),
            "end" => Ok(Key::End),
            "pageup" => Ok(Key::PageUp),
            "pagedown" => Ok(Key::PageDown),
            "tab" => Ok(Key::Tab),
            "enter" | "return" => Ok(Key::Enter),
            "backspace" => Ok(Key::Backspace),
            "delete" => Ok(Key::Delete),
            "escape" | "esc" => Ok(Key::Escape),
            "space" => Ok(Key::Space),
            other if other.chars().count() == 1 => {
                Ok(Key::Char(other.chars().next().expect("one char")))
            }
            other if other.starts_with('f') && other[1..].parse::<u8>().is_ok() => {
                Ok(Key::F(other[1..].parse::<u8>().expect("checked")))
            }
            other => Err(format!("Unknown key: '{other}'")),
        }
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

    /// Every key's canonical name reads back as itself — the property the MCP
    /// key tools rely on to hand a binding straight back as a chord to send.
    #[test]
    fn key_name_round_trip() {
        let all = [
            Key::Cmd,
            Key::Ctrl,
            Key::Alt,
            Key::Shift,
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
            Key::Tab,
            Key::Enter,
            Key::Backspace,
            Key::Delete,
            Key::Escape,
            Key::Space,
            Key::Char('s'),
            Key::F(5),
        ];
        for key in all {
            let name = key.to_string();
            assert_eq!(
                name.parse::<Key>(),
                Ok(key.clone()),
                "key {key:?} does not survive its own name {name:?}"
            );
        }
    }

    #[test]
    fn key_aliases_and_case_are_accepted() {
        assert_eq!("Command".parse::<Key>(), Ok(Key::Cmd));
        assert_eq!("OPTION".parse::<Key>(), Ok(Key::Alt));
        assert_eq!("return".parse::<Key>(), Ok(Key::Enter));
        assert_eq!("esc".parse::<Key>(), Ok(Key::Escape));
        assert!("nope".parse::<Key>().is_err());
    }
}
