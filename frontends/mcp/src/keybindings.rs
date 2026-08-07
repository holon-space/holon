//! The keybinding registry as the MCP tools report it.
//!
//! Two registries decide what a keystroke does: the STRUCTURAL one
//! (`BuilderServices::key_bindings_snapshot` — chords joined into reactive
//! operation descriptors) and the WINDOW one (the frontend's platform keymap:
//! undo, redo, quick-open, tab switching). `list_keybindings` reported only
//! the first, so an agent obeying "read a shortcut before you send it"
//! concluded undo was unbound (dogfood 2026-08-07). This module unions them,
//! keeps the source registry attached to every entry, and answers the one
//! question `send_key_chord` needs: is this chord bound anywhere?

use std::collections::BTreeMap;

use holon_api::KeyChord;

use crate::server::WindowKeyBinding;

/// Which registry a binding came from. An agent that cannot tell them apart
/// cannot tell a chord that is unbound from one that is merely unreported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BindingRegistry {
    /// Chords wired to reactive operations (indent, split_block, …).
    Structural,
    /// Chords registered with the frontend's platform keymap (undo, redo, …).
    Window,
}

impl BindingRegistry {
    pub fn as_str(self) -> &'static str {
        match self {
            BindingRegistry::Structural => "structural",
            BindingRegistry::Window => "window",
        }
    }
}

/// One binding, tagged with the registry that owns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaggedBinding {
    pub action: String,
    pub chord: KeyChord,
    /// Chord keys in the wire vocabulary `send_key_chord` accepts.
    pub keys: Vec<String>,
    pub registry: BindingRegistry,
    /// Keymap context the chord is scoped to; `None` = always active.
    pub context: Option<String>,
}

/// Union the two registries into one snapshot, sorted by action name.
///
/// `window` is `None` in a headless run (no frontend published a keymap) —
/// callers must disclose that rather than present the structural registry as
/// the whole truth. A window chord whose keys fall outside the
/// `holon_api::Key` vocabulary is an `Err`: reporting a chord an agent cannot
/// send back would be worse than not reporting it.
pub fn union_key_bindings(
    structural: BTreeMap<String, KeyChord>,
    window: Option<&[WindowKeyBinding]>,
) -> Result<Vec<TaggedBinding>, String> {
    let mut out: Vec<TaggedBinding> = structural
        .into_iter()
        .map(|(action, chord)| TaggedBinding {
            action,
            keys: chord.0.iter().map(|k| k.to_string()).collect(),
            chord,
            registry: BindingRegistry::Structural,
            context: None,
        })
        .collect();

    for wb in window.unwrap_or(&[]) {
        let keys: Vec<holon_api::Key> = wb
            .keys
            .iter()
            .map(|k| {
                k.parse::<holon_api::Key>().map_err(|e| {
                    format!(
                        "window binding {:?} has key {k:?} outside the wire vocabulary: {e}",
                        wb.action
                    )
                })
            })
            .collect::<Result<_, _>>()?;
        out.push(TaggedBinding {
            action: wb.action.clone(),
            chord: KeyChord::new(&keys),
            keys: wb.keys.clone(),
            registry: BindingRegistry::Window,
            context: wb.context.clone(),
        });
    }

    out.sort_by(|a, b| (a.registry, &a.action).cmp(&(b.registry, &b.action)));
    Ok(out)
}

/// Every binding whose chord equals `chord`. Empty = the chord is bound in
/// neither registry.
pub fn match_chord<'a>(bindings: &'a [TaggedBinding], chord: &KeyChord) -> Vec<&'a TaggedBinding> {
    bindings.iter().filter(|b| &b.chord == chord).collect()
}

#[cfg(test)]
mod tests {
    use holon_api::Key;

    use super::*;

    fn structural() -> BTreeMap<String, KeyChord> {
        BTreeMap::from([
            ("indent".to_string(), KeyChord::new(&[Key::Tab])),
            (
                "outdent".to_string(),
                KeyChord::new(&[Key::Shift, Key::Tab]),
            ),
        ])
    }

    fn window() -> Vec<WindowKeyBinding> {
        vec![
            WindowKeyBinding {
                action: "undo".into(),
                keys: vec!["cmd".into(), "z".into()],
                context: None,
            },
            WindowKeyBinding {
                action: "redo".into(),
                keys: vec!["cmd".into(), "shift".into(), "z".into()],
                context: None,
            },
            WindowKeyBinding {
                action: "turn_into_page".into(),
                keys: vec!["cmd".into(), "shift".into(), "p".into()],
                context: Some("Input".into()),
            },
        ]
    }

    #[test]
    fn the_union_carries_window_chords_the_structural_registry_never_had() {
        let w = window();
        let all = union_key_bindings(structural(), Some(&w)).expect("vocabulary is valid");
        let undo = all
            .iter()
            .find(|b| b.action == "undo")
            .expect("undo is reported");
        assert_eq!(undo.registry, BindingRegistry::Window);
        assert_eq!(undo.chord, KeyChord::new(&[Key::Cmd, Key::Char('z')]));
        assert!(all.iter().any(|b| b.action == "redo"));
        // The structural entries survive the union unchanged.
        let indent = all
            .iter()
            .find(|b| b.action == "indent")
            .expect("indent is reported");
        assert_eq!(indent.registry, BindingRegistry::Structural);
        assert_eq!(indent.keys, vec!["tab".to_string()]);
    }

    #[test]
    fn a_context_scoped_chord_reports_its_context() {
        let w = window();
        let all = union_key_bindings(structural(), Some(&w)).expect("vocabulary is valid");
        let tip = all
            .iter()
            .find(|b| b.action == "turn_into_page")
            .expect("reported");
        assert_eq!(tip.context.as_deref(), Some("Input"));
    }

    #[test]
    fn a_window_chord_matches_the_same_keys_a_caller_would_send() {
        let w = window();
        let all = union_key_bindings(structural(), Some(&w)).expect("vocabulary is valid");
        let sent = KeyChord::new(&[Key::Cmd, Key::Char('z')]);
        let hits = match_chord(&all, &sent);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].action, "undo");
    }

    #[test]
    fn an_unbound_chord_matches_nothing_and_a_bound_one_matches() {
        let w = window();
        let all = union_key_bindings(structural(), Some(&w)).expect("vocabulary is valid");
        assert!(match_chord(&all, &KeyChord::new(&[Key::Tab])).len() == 1);
        assert!(match_chord(&all, &KeyChord::new(&[Key::F(7)])).is_empty());
    }

    #[test]
    fn a_window_chord_outside_the_wire_vocabulary_is_an_error_not_a_silent_drop() {
        let bad = vec![WindowKeyBinding {
            action: "bogus".into(),
            keys: vec!["hyper".into()],
            context: None,
        }];
        let err = union_key_bindings(structural(), Some(&bad)).expect_err("must fail loud");
        assert!(err.contains("bogus"), "{err}");
    }

    #[test]
    fn a_headless_run_reports_only_the_structural_registry() {
        let all = union_key_bindings(structural(), None).expect("vocabulary is valid");
        assert_eq!(all.len(), 2);
        assert!(
            all.iter()
                .all(|b| b.registry == BindingRegistry::Structural)
        );
    }
}
