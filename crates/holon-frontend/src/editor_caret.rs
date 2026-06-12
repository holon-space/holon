//! Pure caret / text arithmetic — the byte-offset, UTF-8-boundary math the
//! editor runs, with **no** engine, Loro, or `MutableText` coupling.
//!
//! This is the single source of truth shared by two callers that would
//! otherwise re-implement it:
//! - production [`crate::headless_editor_mirror::HeadlessEditorMirror`] uses the
//!   position primitives (`move_left`/`move_right`/`clamp_boundary`) and the
//!   byte→codepoint conversions ([`byte_to_codepoint`]/[`codepoint_len`]) it
//!   needs to build Loro's codepoint-indexed `TextOp`s;
//! - the PBT in-memory editor component uses the string mutators
//!   ([`insert_at`]/[`delete_back`]), which are themselves built on the same
//!   position primitives.
//!
//! Keeping this pure means the editor PBT slice exercises the *real* caret math
//! (the component is a thin wrapper, not a parallel copy), and the triplicated
//! inline `chars().count()` byte↔codepoint conversions live in one place.
//!
//! Caret positions are **byte** offsets into `text` and are assumed to sit on a
//! char boundary (every function here preserves that invariant).

/// Byte offset one char to the **right** of `caret`, clamped at end-of-text
/// (returns `caret` unchanged when already at the end).
pub fn move_right(text: &str, caret: usize) -> usize {
    let advance = text[caret..]
        .chars()
        .next()
        .map(char::len_utf8)
        .unwrap_or(0);
    caret + advance
}

/// Byte offset one char to the **left** of `caret`, clamped at 0 (returns
/// `caret` unchanged when already at the start).
pub fn move_left(text: &str, caret: usize) -> usize {
    let retreat = text[..caret]
        .chars()
        .next_back()
        .map(char::len_utf8)
        .unwrap_or(0);
    caret - retreat
}

/// Largest char boundary `<= byte.min(text.len())` — where a caret lands when a
/// raw byte target (`home`/`end`/a click) might fall mid-codepoint.
pub fn clamp_boundary(text: &str, byte: usize) -> usize {
    let mut b = byte.min(text.len());
    while !text.is_char_boundary(b) {
        b -= 1;
    }
    b
}

/// Number of codepoints before byte offset `byte` — the index Loro's
/// codepoint-addressed `TextOp`s expect. `byte` must be a char boundary.
pub fn byte_to_codepoint(text: &str, byte: usize) -> usize {
    text[..byte].chars().count()
}

/// Number of codepoints in the byte range `start..end` (both char boundaries) —
/// the `len_codepoint` of a Loro `TextOp::Delete`.
pub fn codepoint_len(text: &str, start: usize, end: usize) -> usize {
    text[start..end].chars().count()
}

/// Insert `s` at `caret` (a char boundary) and return the advanced caret. The
/// string-owning analog of production's `MutableText` insert + `caret + len`.
pub fn insert_at(text: &mut String, caret: usize, s: &str) -> usize {
    text.insert_str(caret, s);
    caret + s.len()
}

/// Delete up to `n` chars before `caret` (codepoint-wise, via [`move_left`]) and
/// return the new caret. The string-owning analog of production's repeated
/// mid-line backspace.
pub fn delete_back(text: &mut String, caret: usize, n: usize) -> usize {
    let mut start = caret;
    for _ in 0..n {
        let prev = move_left(text, start);
        if prev == start {
            break;
        }
        start = prev;
    }
    text.replace_range(start..caret, "");
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    // "héllo": h=1, é=2, l=1, l=1, o=1 → bytes [0,1,3,4,5,6]; boundaries at
    // 0,1,3,4,5,6.
    const MULTI: &str = "héllo";

    #[test]
    fn move_right_steps_one_codepoint_over_multibyte() {
        assert_eq!(move_right(MULTI, 0), 1); // over 'h'
        assert_eq!(move_right(MULTI, 1), 3); // over 'é' (2 bytes)
        assert_eq!(move_right(MULTI, 6), 6); // clamp at end
    }

    #[test]
    fn move_left_steps_one_codepoint_over_multibyte() {
        assert_eq!(move_left(MULTI, 3), 1); // back over 'é'
        assert_eq!(move_left(MULTI, 1), 0); // back over 'h'
        assert_eq!(move_left(MULTI, 0), 0); // clamp at start
    }

    #[test]
    fn clamp_boundary_walks_back_off_mid_codepoint() {
        assert_eq!(clamp_boundary(MULTI, 2), 1); // mid-'é' → start of 'é'
        assert_eq!(clamp_boundary(MULTI, 100), 6); // past end → end
        assert_eq!(clamp_boundary(MULTI, 3), 3); // already a boundary
    }

    #[test]
    fn byte_codepoint_conversions() {
        assert_eq!(byte_to_codepoint(MULTI, 3), 2); // 'h','é' before byte 3
        assert_eq!(codepoint_len(MULTI, 1, 3), 1); // just 'é'
    }

    #[test]
    fn insert_at_advances_by_byte_len() {
        let mut t = "ab".to_string();
        let caret = insert_at(&mut t, 1, "é");
        assert_eq!(t, "aéb");
        assert_eq!(caret, 1 + "é".len()); // advanced by 2 bytes
    }

    #[test]
    fn delete_back_removes_codepoints_not_bytes() {
        let mut t = "aéb".to_string();
        let caret = delete_back(&mut t, 3, 1); // caret after 'é' (byte 3)
        assert_eq!(t, "ab");
        assert_eq!(caret, 1);
    }

    #[test]
    fn delete_back_clamps_at_start() {
        let mut t = "aé".to_string();
        let caret = delete_back(&mut t, 3, 5); // ask for more than exist
        assert_eq!(t, "");
        assert_eq!(caret, 0);
    }

    // ── Unit-PBT: the durable guard for the single editor-text primitive ──────
    //
    // Both the SUT (`InMemEditorComponent`) and the reference oracle
    // (`ReferenceState::ActiveEditor`) drive THIS module for caret/text math.
    // Text-primitive correctness therefore lives here, not in any SUT-vs-ref
    // differential. The properties below hold for any op sequence over a
    // multibyte alphabet (ASCII + accented + a 4-byte emoji), so caret bugs
    // (mid-codepoint landings, byte-vs-char miscounts) surface as a panic here
    // long before they could mask a real divergence upstream.
    mod prop {
        use super::*;
        use proptest::prelude::*;

        /// Multibyte alphabet: 1-byte ASCII, a 2-byte accented char, and a
        /// 4-byte emoji — exercises every char-width the caret math must handle.
        const ALPHABET: &[&str] = &["a", "b", " ", "é", "😀"];

        #[derive(Debug, Clone)]
        enum Op {
            Insert(String),
            DeleteBack(usize),
            MoveLeft,
            MoveRight,
            /// A raw byte target that may land mid-codepoint (home/end/click).
            ClampTo(usize),
        }

        fn op_strategy() -> impl Strategy<Value = Op> {
            prop_oneof![
                proptest::collection::vec(proptest::sample::select(ALPHABET), 1..4)
                    .prop_map(|cs| Op::Insert(cs.concat())),
                (1usize..4).prop_map(Op::DeleteBack),
                Just(Op::MoveLeft),
                Just(Op::MoveRight),
                (0usize..32).prop_map(Op::ClampTo),
            ]
        }

        /// `caret` is always a char boundary of `text` (precondition every
        /// primitive relies on — violating it panics `String` indexing).
        fn assert_invariants(text: &str, caret: usize) {
            assert!(
                text.is_char_boundary(caret),
                "caret {caret} not on a char boundary of {text:?}"
            );
            assert!(caret <= text.len(), "caret {caret} past end of {text:?}");
            // `text` is a `String`, so it is valid UTF-8 by construction; this
            // re-asserts it survived the op (a defensive trip-wire on the type).
            assert!(std::str::from_utf8(text.as_bytes()).is_ok());
        }

        proptest! {
            #[test]
            fn caret_stays_on_a_boundary_through_any_op_sequence(
                ops in proptest::collection::vec(op_strategy(), 0..40)
            ) {
                let mut text = String::new();
                let mut caret = 0usize;
                for op in ops {
                    match op {
                        Op::Insert(s) => caret = insert_at(&mut text, caret, &s),
                        Op::DeleteBack(n) => caret = delete_back(&mut text, caret, n),
                        Op::MoveLeft => caret = move_left(&text, caret),
                        Op::MoveRight => caret = move_right(&text, caret),
                        Op::ClampTo(b) => caret = clamp_boundary(&text, b),
                    }
                    assert_invariants(&text, caret);
                }
            }

            /// `delete_back(insert_at(t, c, s), …, |s|_codepoints)` restores the
            /// original text and caret — the editor's fundamental round-trip.
            #[test]
            fn insert_then_delete_round_trips(
                prefix in "[ab é]{0,8}",
                inserted in proptest::collection::vec(proptest::sample::select(ALPHABET), 1..5),
            ) {
                let inserted: String = inserted.concat();
                let codepoints = inserted.chars().count();
                let mut text = prefix.clone();
                let start = text.len();
                let caret = insert_at(&mut text, start, &inserted);
                prop_assert_eq!(&text[start..caret], inserted.as_str());
                let caret = delete_back(&mut text, caret, codepoints);
                prop_assert_eq!(text, prefix);
                prop_assert_eq!(caret, start);
            }

            /// `clamp_boundary` of ANY byte (even mid-codepoint, even past the
            /// end) is always a legal char boundary `<= len`.
            #[test]
            fn clamp_boundary_always_legal(
                text in "[ab é😀]{0,12}",
                byte in 0usize..40,
            ) {
                let c = clamp_boundary(&text, byte);
                prop_assert!(text.is_char_boundary(c));
                prop_assert!(c <= text.len());
                prop_assert!(c <= byte.min(text.len()));
            }
        }
    }
}
