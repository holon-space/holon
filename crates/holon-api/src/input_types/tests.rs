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
