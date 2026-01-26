//! YAML-driven keybinding table for the TUI input handler.
//!
//! Source of truth: `assets/default/keybindings.yaml`, embedded via
//! `include_str!`. Parsed once at startup into a lookup table indexed by
//! `(BindingMode, KeyMatch)`. The input handler consults the table first
//! and falls back to legacy hardcoded matches for actions not yet in the
//! YAML.
//!
//! The schema is:
//!
//! ```yaml
//! bindings:
//!   - key: "h"
//!     modifiers: ["leader"]   # optional; ["ctrl"] | ["alt"] | ["leader"] | omitted
//!     context: "navigation"   # "navigation" | "editing"
//!     action: "go_home"
//! ```
//!
//! Recognized special keys: `Up`, `Down`, `Left`, `Right`, `Enter`,
//! `Tab`, `Esc`, `Backspace`. Anything else is interpreted as a literal
//! character (length 1).
//!
//! Why YAML instead of code: the bindings are read by tests and humans
//! more often than the dispatch logic, and centralising them lets a
//! reviewer tell at a glance what keys do without grepping match arms.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

const KEYBINDINGS_YAML: &str = include_str!("../../../assets/default/keybindings.yaml");

/// Parsing context — decides which dispatch path runs the matched
/// action. Matches the YAML's `context:` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindingMode {
    Navigation,
    Editing,
}

impl BindingMode {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "navigation" => Some(Self::Navigation),
            "editing" => Some(Self::Editing),
            _ => None,
        }
    }
}

/// Concrete key shape we hash on for lookup. `Special` covers the
/// non-character keys; `Char` carries a single character. Modifiers are
/// stored as an explicit set rather than bitfields so the lookup key
/// stays canonical regardless of source order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyMatch {
    pub key: KeyKind,
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyKind {
    Char(char),
    Special(SpecialKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecialKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Tab,
    Esc,
    Backspace,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// True iff the chord must be entered after pressing the leader
    /// (Space) in navigation mode.
    pub leader: bool,
}

#[derive(Debug, Deserialize)]
struct YamlFile {
    bindings: Vec<YamlBinding>,
}

#[derive(Debug, Deserialize)]
struct YamlBinding {
    key: String,
    #[serde(default)]
    modifiers: Vec<String>,
    context: String,
    action: String,
}

/// In-memory binding table built from the YAML.
#[derive(Debug)]
pub struct Bindings {
    by_mode: HashMap<BindingMode, HashMap<KeyMatch, String>>,
}

impl Bindings {
    /// Look up the action bound to `(mode, key)`. Returns `None` if no
    /// rule matches; the caller falls back to legacy hardcoded handling.
    pub fn action_for(&self, mode: BindingMode, key: &KeyMatch) -> Option<&str> {
        self.by_mode.get(&mode)?.get(key).map(String::as_str)
    }
}

fn parse_special(s: &str) -> Option<SpecialKey> {
    Some(match s {
        "Up" => SpecialKey::Up,
        "Down" => SpecialKey::Down,
        "Left" => SpecialKey::Left,
        "Right" => SpecialKey::Right,
        "Enter" => SpecialKey::Enter,
        "Tab" => SpecialKey::Tab,
        "Esc" | "Escape" => SpecialKey::Esc,
        "Backspace" => SpecialKey::Backspace,
        _ => return None,
    })
}

fn parse_modifiers(raw: &[String]) -> Result<Modifiers, String> {
    let mut m = Modifiers::default();
    for raw_mod in raw {
        match raw_mod.as_str() {
            "ctrl" => m.ctrl = true,
            "alt" => m.alt = true,
            "shift" => m.shift = true,
            "leader" => m.leader = true,
            other => return Err(format!("unknown modifier '{other}' in keybindings.yaml")),
        }
    }
    Ok(m)
}

fn parse_key(s: &str) -> Result<KeyKind, String> {
    if let Some(sk) = parse_special(s) {
        return Ok(KeyKind::Special(sk));
    }
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Ok(KeyKind::Char(c)),
        _ => Err(format!(
            "key '{s}' is neither a recognised special key nor a single character"
        )),
    }
}

fn build() -> Result<Bindings, String> {
    let parsed: YamlFile = serde_yaml::from_str(KEYBINDINGS_YAML)
        .map_err(|e| format!("keybindings.yaml: parse error: {e}"))?;
    let mut by_mode: HashMap<BindingMode, HashMap<KeyMatch, String>> = HashMap::new();
    for b in parsed.bindings {
        let mode = BindingMode::parse(&b.context)
            .ok_or_else(|| format!("keybindings.yaml: unknown context '{}'", b.context))?;
        let modifiers = parse_modifiers(&b.modifiers)?;
        let key = parse_key(&b.key)?;
        let key_match = KeyMatch { key, modifiers };
        let bucket = by_mode.entry(mode).or_default();
        if let Some(existing) = bucket.insert(key_match.clone(), b.action.clone()) {
            return Err(format!(
                "keybindings.yaml: duplicate binding for {:?} in {:?} \
                 (was '{existing}', now '{}')",
                key_match, mode, b.action
            ));
        }
    }
    Ok(Bindings { by_mode })
}

/// Lazily-initialised, process-wide binding table. Built on first
/// access and reused for the lifetime of the process. Panics if the
/// embedded YAML fails to parse — that's a build-time concern caught
/// by the unit test below, not a runtime concern.
pub fn global() -> &'static Bindings {
    static BINDINGS: OnceLock<Bindings> = OnceLock::new();
    BINDINGS.get_or_init(|| build().expect("keybindings.yaml must parse"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_parses_cleanly() {
        let b = build().expect("keybindings.yaml should parse");
        assert!(b.by_mode.contains_key(&BindingMode::Navigation));
    }

    #[test]
    fn nav_history_bindings_present() {
        let b = global();
        let leader_h = KeyMatch {
            key: KeyKind::Char('h'),
            modifiers: Modifiers {
                leader: true,
                ..Default::default()
            },
        };
        let leader_b = KeyMatch {
            key: KeyKind::Char('b'),
            modifiers: Modifiers {
                leader: true,
                ..Default::default()
            },
        };
        let leader_f = KeyMatch {
            key: KeyKind::Char('f'),
            modifiers: Modifiers {
                leader: true,
                ..Default::default()
            },
        };
        assert_eq!(
            b.action_for(BindingMode::Navigation, &leader_h),
            Some("go_home")
        );
        assert_eq!(
            b.action_for(BindingMode::Navigation, &leader_b),
            Some("go_back")
        );
        assert_eq!(
            b.action_for(BindingMode::Navigation, &leader_f),
            Some("go_forward")
        );
    }

    #[test]
    fn existing_leader_chords_present() {
        let b = global();
        for (chord, expected) in [
            ('x', "cycle_task_state"),
            // arrows are SpecialKey, not Char, so they'd fail this loop
        ] {
            let km = KeyMatch {
                key: KeyKind::Char(chord),
                modifiers: Modifiers {
                    leader: true,
                    ..Default::default()
                },
            };
            assert_eq!(b.action_for(BindingMode::Navigation, &km), Some(expected));
        }
    }
}
