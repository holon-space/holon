//! Leader-chord keybinding lookup for the wide-PBT chord dispatcher.
//!
//! Embeds `assets/default/keybindings.yaml` at compile time and exposes
//! [`leader_key_for`] so `E2ESut::send_leader_chord` can resolve nav-op
//! names → keystrokes without going through a runtime config load.
//! Extracted from `sut.rs` (Phase D1).

/// Look up the single-character leader-chord key bound to a navigation
/// op in `assets/default/keybindings.yaml`. Embeds the YAML at compile
/// time so the test stays in lock-step with the canonical binding
/// source — moving "h" → "g" for `go_home` in the YAML moves the chord
/// key the test sends.
///
/// Returns the literal key string (e.g. `"h"` for `go_home`). Panics if
/// no leader-chord binding for `nav_op` is present — that's a
/// programming error in either the YAML or the caller, not a runtime
/// condition.
pub(super) fn leader_key_for(nav_op: &str) -> &'static str {
    const YAML: &str = include_str!("../../../../assets/default/keybindings.yaml");
    static PARSED: std::sync::OnceLock<KeybindingsFile> = std::sync::OnceLock::new();
    let parsed = PARSED.get_or_init(|| {
        serde_yaml::from_str(YAML).expect("assets/default/keybindings.yaml must parse")
    });
    let key = parsed.bindings.iter().find_map(|b| {
        if b.context == "navigation"
            && b.action == nav_op
            && b.modifiers.iter().any(|m| m == "leader")
        {
            Some(b.key.as_str())
        } else {
            None
        }
    });
    key.unwrap_or_else(|| {
        panic!(
            "no leader-chord binding for nav_op '{nav_op}' in \
             assets/default/keybindings.yaml — caller passed a name \
             that doesn't appear in the YAML, or the YAML lost that \
             binding"
        )
    })
}

#[derive(serde::Deserialize)]
struct KeybindingsFile {
    bindings: Vec<KeybindingEntry>,
}

#[derive(serde::Deserialize)]
struct KeybindingEntry {
    key: String,
    #[serde(default)]
    modifiers: Vec<String>,
    context: String,
    action: String,
}

#[cfg(test)]
mod leader_key_tests {
    use super::leader_key_for;

    #[test]
    fn yaml_leader_bindings_resolve_to_expected_keys() {
        assert_eq!(leader_key_for("go_home"), "h");
        assert_eq!(leader_key_for("go_back"), "b");
        assert_eq!(leader_key_for("go_forward"), "f");
    }

    #[test]
    #[should_panic(expected = "no leader-chord binding")]
    fn unknown_nav_op_panics() {
        leader_key_for("not_a_real_op");
    }

    /// Direct-binding (no `modifiers: ["leader"]`) actions like
    /// `start_editing` (Enter) must NOT match — `send_leader_chord` is
    /// only for leader-prefixed chords.
    #[test]
    #[should_panic(expected = "no leader-chord binding")]
    fn direct_non_leader_binding_does_not_match() {
        // `start_editing` is bound to `Enter` directly AND under leader.
        // The leader binding wins (matches first); but a direct-only op
        // would panic. `switch_region` is direct-only on Tab.
        leader_key_for("switch_region");
    }
}
