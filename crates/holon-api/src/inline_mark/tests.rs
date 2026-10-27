use super::*;

fn link(name: &str) -> InlineMark {
    InlineMark::Link {
        target: EntityRef::Name {
            name: name.to_string(),
        },
        label: name.to_string(),
    }
}

#[test]
fn split_keeps_link_entirely_left_and_leaves_right_bare() {
    // Dogfood 2026-07-20 repro: "Owner is Ada Lovelace and reviewer",
    // link over "Ada Lovelace" = scalars [9, 21). Split after "and"
    // (byte 25, before the space). Link is wholly left → preserved in
    // bounds; right half "reviewer" carries no marks.
    let content = "Owner is Ada Lovelace and reviewer";
    let marks = vec![MarkSpan::new(9, 21, link("Ada Lovelace"))];
    let out = split_content_marks(content, &marks, 25);
    assert_eq!(out.left.content, "Owner is Ada Lovelace and");
    assert_eq!(
        out.left.marks,
        vec![MarkSpan::new(9, 21, link("Ada Lovelace"))]
    );
    assert_eq!(out.right.content, "reviewer");
    assert!(out.right.marks.is_empty());
}

#[test]
fn split_moves_link_entirely_right_rebased() {
    // "see Ada Lovelace" split after "see " (byte 4): link over
    // "Ada Lovelace" [4, 16) moves right, rebased to [0, 12).
    let content = "see Ada Lovelace";
    let marks = vec![MarkSpan::new(4, 16, link("Ada Lovelace"))];
    let out = split_content_marks(content, &marks, 4);
    assert_eq!(out.left.content, "see");
    assert!(out.left.marks.is_empty());
    assert_eq!(out.right.content, "Ada Lovelace");
    assert_eq!(
        out.right.marks,
        vec![MarkSpan::new(0, 12, link("Ada Lovelace"))]
    );
}

#[test]
fn split_inside_link_degrades_to_plain_text_both_sides() {
    // Split INSIDE "Ada Lovelace" (byte 12 = after "Ada Lovel") — the link
    // straddles the split, so it degrades to plain text on BOTH halves.
    let content = "Ada Lovelace";
    let marks = vec![MarkSpan::new(0, 12, link("Ada Lovelace"))];
    let out = split_content_marks(content, &marks, 5); // "Ada L" | "ovelace"
    assert_eq!(out.left.content, "Ada L");
    assert!(
        out.left.marks.is_empty(),
        "left link remnant must be dropped, got {:?}",
        out.left.marks
    );
    assert_eq!(out.right.content, "ovelace");
    assert!(
        out.right.marks.is_empty(),
        "right link remnant must be dropped, got {:?}",
        out.right.marks
    );
}

#[test]
fn split_inside_formatting_mark_truncates_both_sides() {
    // Bold over all of "bold" [0,4); split after "bo" (byte 2).
    let content = "bold";
    let marks = vec![MarkSpan::new(0, 4, InlineMark::Bold)];
    let out = split_content_marks(content, &marks, 2);
    assert_eq!(out.left.content, "bo");
    assert_eq!(out.left.marks, vec![MarkSpan::new(0, 2, InlineMark::Bold)]);
    assert_eq!(out.right.content, "ld");
    assert_eq!(out.right.marks, vec![MarkSpan::new(0, 2, InlineMark::Bold)]);
}

#[test]
fn split_at_start_moves_all_marks_right() {
    let content = "Ada";
    let marks = vec![MarkSpan::new(0, 3, InlineMark::Italic)];
    let out = split_content_marks(content, &marks, 0);
    assert_eq!(out.left.content, "");
    assert!(out.left.marks.is_empty());
    assert_eq!(out.right.content, "Ada");
    assert_eq!(
        out.right.marks,
        vec![MarkSpan::new(0, 3, InlineMark::Italic)]
    );
}

#[test]
fn split_at_end_keeps_all_marks_left() {
    let content = "Ada";
    let marks = vec![MarkSpan::new(0, 3, InlineMark::Italic)];
    let out = split_content_marks(content, &marks, 3);
    assert_eq!(out.left.content, "Ada");
    assert_eq!(
        out.left.marks,
        vec![MarkSpan::new(0, 3, InlineMark::Italic)]
    );
    assert_eq!(out.right.content, "");
    assert!(out.right.marks.is_empty());
}

#[test]
fn split_clamps_left_mark_into_trimmed_trailing_whitespace() {
    // Bold covers "ab  " [0,4) including two trailing spaces; splitting at
    // byte 4 trims them off the left, so the bold clamps to [0,2).
    let content = "ab  cd";
    let marks = vec![MarkSpan::new(0, 4, InlineMark::Bold)];
    let out = split_content_marks(content, &marks, 4);
    assert_eq!(out.left.content, "ab");
    assert_eq!(out.left.marks, vec![MarkSpan::new(0, 2, InlineMark::Bold)]);
    assert_eq!(out.right.content, "cd");
}

#[test]
fn split_boundary_marks_do_not_cross() {
    // Two adjacent bolds [0,3) and [3,6); split exactly at scalar 3.
    let content = "abcdef";
    let marks = vec![
        MarkSpan::new(0, 3, InlineMark::Bold),
        MarkSpan::new(3, 6, InlineMark::Italic),
    ];
    let out = split_content_marks(content, &marks, 3);
    assert_eq!(out.left.marks, vec![MarkSpan::new(0, 3, InlineMark::Bold)]);
    assert_eq!(
        out.right.marks,
        vec![MarkSpan::new(0, 3, InlineMark::Italic)]
    );
}

#[test]
fn split_multibyte_prefix_rebases_right_mark_in_scalar_space() {
    // "über Ada tail": ü is 2 bytes, so BYTE and SCALAR offsets diverge.
    // Link over "Ada" = scalars [5,8). Split after "über " (BYTE 6, which
    // is 5 SCALARS). The link is entirely right and must rebase to [0,3) in
    // scalar space — a byte-space rebase (by 6) would give [-1,2)/[0,2).
    let content = "über Ada tail";
    assert_eq!(content.len(), 14, "ü is 2 bytes: 14 bytes, 13 scalars");
    let marks = vec![MarkSpan::new(5, 8, link("Ada"))];
    let out = split_content_marks(content, &marks, 6);
    assert_eq!(out.left.content, "über");
    assert!(out.left.marks.is_empty());
    assert_eq!(out.right.content, "Ada tail");
    assert_eq!(out.right.marks, vec![MarkSpan::new(0, 3, link("Ada"))]);
}

#[test]
fn split_end_clamps_left_mark_spanning_multibyte_chars() {
    // "café  bar": é is 2 bytes. Bold covers "café  " = scalars [0,6)
    // (including the two trailing spaces). Splitting after the spaces
    // (BYTE 7, = 6 SCALARS) trims them off the left, clamping the bold to
    // [0,4) over "café" — the clamp is computed in scalar space across é.
    let content = "café  bar";
    assert_eq!(content.len(), 10, "café=5 bytes + 2 spaces + bar=3");
    let marks = vec![MarkSpan::new(0, 6, InlineMark::Bold)];
    let out = split_content_marks(content, &marks, 7);
    assert_eq!(out.left.content, "café");
    assert_eq!(out.left.marks, vec![MarkSpan::new(0, 4, InlineMark::Bold)]);
    assert_eq!(out.right.content, "bar");
    assert!(out.right.marks.is_empty());
}

#[test]
fn split_at_byte_boundary_right_after_multibyte_char() {
    // Split lands on the BYTE boundary immediately after é (byte 5), which
    // is SCALAR 4. A bold ending exactly at é stays fully left (end-clamped
    // in scalar space); a link in the right half rebases to [0,3).
    let content = "café bar";
    assert_eq!(content.len(), 9, "café=5 bytes + space + bar=3");
    assert!(
        content.is_char_boundary(5),
        "byte 5 is the boundary after é"
    );
    let marks = vec![
        MarkSpan::new(0, 4, InlineMark::Bold), // "café"
        MarkSpan::new(5, 8, link("bar")),      // "bar"
    ];
    let out = split_content_marks(content, &marks, 5);
    assert_eq!(out.left.content, "café");
    assert_eq!(out.left.marks, vec![MarkSpan::new(0, 4, InlineMark::Bold)]);
    assert_eq!(out.right.content, "bar");
    assert_eq!(out.right.marks, vec![MarkSpan::new(0, 3, link("bar"))]);
}

#[test]
fn round_trip_simple_marks() {
    let marks = vec![
        MarkSpan::new(0, 5, InlineMark::Bold),
        MarkSpan::new(5, 10, InlineMark::Italic),
    ];
    let json = marks_to_json(&marks);
    let back = marks_from_json(&json).expect("round-trip");
    assert_eq!(marks, back);
}

#[test]
fn canonicalize_makes_equal_sets_compare_equal_regardless_of_input_order() {
    // Same mark set, two emission orders (as a Loro HashMap scan vs a SQL
    // JSON column might produce). After canonicalization they must be
    // identical so the projection's `Vec<MarkSpan>` compare is a no-op.
    let a = vec![
        MarkSpan::new(6, 11, InlineMark::Italic),
        MarkSpan::new(0, 5, InlineMark::Bold),
        MarkSpan::new(0, 5, InlineMark::Underline),
    ];
    let b = vec![
        MarkSpan::new(0, 5, InlineMark::Underline),
        MarkSpan::new(0, 5, InlineMark::Bold),
        MarkSpan::new(6, 11, InlineMark::Italic),
    ];
    let mut a_canon = a.clone();
    let mut b_canon = b.clone();
    canonicalize_marks(&mut a_canon);
    canonicalize_marks(&mut b_canon);
    assert_eq!(a_canon, b_canon);
    // And the order is the expected (start, end, kind) order.
    assert_eq!(a_canon[0], MarkSpan::new(0, 5, InlineMark::Bold));
    assert_eq!(a_canon[1], MarkSpan::new(0, 5, InlineMark::Underline));
    assert_eq!(a_canon[2], MarkSpan::new(6, 11, InlineMark::Italic));
}

#[test]
fn canonicalize_drops_zero_width_marks() {
    // A collapsed mark (start == end) is left behind when an edit deletes
    // the entire text under a mark (e.g. a link's label). Every read
    // boundary routes through canonicalize_marks, which must strip it so it
    // never reaches a projection or the org writeback (`]][[` root cause).
    let mut marks = vec![
        MarkSpan::new(0, 4, InlineMark::Bold),
        MarkSpan::new(2, 2, InlineMark::Italic),
        MarkSpan::new(
            4,
            4,
            InlineMark::Link {
                target: EntityRef::Name {
                    name: String::new(),
                },
                label: String::new(),
            },
        ),
    ];
    canonicalize_marks(&mut marks);
    assert_eq!(marks, vec![MarkSpan::new(0, 4, InlineMark::Bold)]);

    // marks_from_json (a read boundary) inherits the drop.
    let json = marks_to_json(&[MarkSpan::new(0, 0, InlineMark::Bold)]);
    let back = marks_from_json(&json).expect("parse");
    assert!(
        back.is_empty(),
        "zero-width mark survived json read: {back:?}"
    );
}

#[test]
fn canonicalize_against_clamps_out_of_bounds_end() {
    // A mark whose end outlives the content (a decoupled marks-only write,
    // or a later content-only trim) must be clamped to the content length —
    // otherwise it aborts every render in scalar_range_to_bytes.
    let mut marks = vec![MarkSpan::new(0, 5, InlineMark::Bold)];
    canonicalize_marks_against("abc", &mut marks, "block:test"); // "abc" = 3 scalars
    assert_eq!(marks, vec![MarkSpan::new(0, 3, InlineMark::Bold)]);
}

#[test]
fn canonicalize_against_drops_fully_out_of_bounds_mark() {
    // start and end both past the content clamp to (len, len) = empty, which
    // canonicalize then drops.
    let mut marks = vec![MarkSpan::new(4, 6, InlineMark::Italic)];
    canonicalize_marks_against("ab", &mut marks, "block:test"); // "ab" = 2 scalars
    assert!(
        marks.is_empty(),
        "fully out-of-bounds mark survived: {marks:?}"
    );
}

#[test]
fn canonicalize_against_uses_scalar_not_byte_length() {
    // Multi-byte content: a 3-scalar string ("a你好") whose byte length is 7.
    // A mark 0..3 is in-bounds by SCALARS and must be preserved untouched.
    let mut marks = vec![MarkSpan::new(0, 3, InlineMark::Bold)];
    canonicalize_marks_against("a你好", &mut marks, "block:test");
    assert_eq!(marks, vec![MarkSpan::new(0, 3, InlineMark::Bold)]);
}

#[test]
fn round_trip_link_external() {
    let marks = vec![MarkSpan::new(
        0,
        4,
        InlineMark::Link {
            target: EntityRef::External {
                url: "https://example.com".to_string(),
            },
            label: "demo".to_string(),
        },
    )];
    let json = marks_to_json(&marks);
    let back = marks_from_json(&json).expect("round-trip");
    assert_eq!(marks, back);
}

#[test]
fn round_trip_link_internal() {
    let marks = vec![MarkSpan::new(
        10,
        20,
        InlineMark::Link {
            target: EntityRef::from_uri(&EntityUri::block("abc-123")),
            label: "see also".to_string(),
        },
    )];
    let json = marks_to_json(&marks);
    let back = marks_from_json(&json).expect("round-trip");
    assert_eq!(marks, back);
}

#[test]
fn loro_keys_cover_all_variants() {
    // Every variant must produce a key, and `all_loro_keys` must list
    // every key exactly once. This is the gate for `config_text_style`
    // installing the right `ExpandType` for every variant the editor
    // can produce.
    let cases = [
        InlineMark::Bold,
        InlineMark::Italic,
        InlineMark::Code,
        InlineMark::Verbatim,
        InlineMark::Strike,
        InlineMark::Underline,
        InlineMark::Link {
            target: EntityRef::External { url: String::new() },
            label: String::new(),
        },
        InlineMark::Sub,
        InlineMark::Super,
    ];
    let keys: Vec<&'static str> = cases.iter().map(|m| m.loro_key()).collect();
    let all = InlineMark::all_loro_keys();
    assert_eq!(keys.len(), all.len(), "key count mismatch");
    for k in &keys {
        assert!(all.contains(k), "{k} missing from all_loro_keys");
    }
}

/// The serde `type` tag and the Loro backend's hand-written tag are two
/// independent spellings of ONE wire name. They drifted once
/// (`unknownscheme` vs `unknown_scheme`); pin every variant so they can't
/// again.
#[test]
fn entity_ref_serde_tags_are_the_loro_wire_names() {
    let cases = [
        (
            EntityRef::External {
                url: "https://x".into(),
            },
            "external",
        ),
        (
            EntityRef::from_uri(&EntityUri::parse("block:x").expect("valid uri")),
            "scheme",
        ),
        (EntityRef::Name { name: "P".into() }, "name"),
        (
            // A scheme nothing has registered carries the SAME tag: whether
            // it resolves is asked of the registry at read time, never
            // stored.
            EntityRef::from_uri(&EntityUri::parse("t-widget:abc").expect("valid uri")),
            "scheme",
        ),
    ];
    for (value, expected_tag) in cases {
        let json = serde_json::to_value(&value).expect("EntityRef serializes");
        assert_eq!(
            json.get("type").and_then(|t| t.as_str()),
            Some(expected_tag),
            "wire tag drifted for {value:?}"
        );
    }
}

/// Every legacy payload must load, INCLUDING the ones whose target is not
/// a parseable absolute URI.
///
/// `LinkTarget::UnknownScheme` covers two structural shapes — a whole
/// scheme-shaped target (`t-widget:abc`) and a wiki path with a colon in a
/// later segment (`Areas/cc-session:abc`) — so marks written before the
/// merge hold both. The second is not a valid URI, so storing this variant
/// as an `EntityUri` makes those marks either fail to load or get silently
/// rewritten to `block:…`. The raw target is authored text; it round-trips
/// verbatim or the vault is corrupted.
#[test]
fn legacy_mark_payloads_load_verbatim() {
    let cases = [
        (
            r#"{"type":"unknown_scheme","uri":"Areas/cc-session:abc"}"#,
            "Areas/cc-session:abc",
        ),
        (
            r#"{"type":"unknown_scheme","uri":"Meeting/Notes:2026"}"#,
            "Meeting/Notes:2026",
        ),
        (
            r#"{"type":"unknown_scheme","uri":"t-widget:abc123"}"#,
            "t-widget:abc123",
        ),
        (
            r#"{"type":"internal","id":"block:abc-123"}"#,
            "block:abc-123",
        ),
        (r#"{"type":"scheme","raw":"tag:work"}"#, "tag:work"),
    ];
    for (json, expected_raw) in cases {
        let got: EntityRef = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("legacy payload must load: {json} — {e}"));
        let EntityRef::Scheme { raw } = &got else {
            panic!("expected Scheme for {json}, got {got:?}");
        };
        assert_eq!(raw, expected_raw, "target text was altered loading {json}");
        // And it must survive a re-serialize unchanged.
        let reser = serde_json::to_value(&got).expect("serializes");
        assert_eq!(reser.get("type").and_then(|t| t.as_str()), Some("scheme"));
        assert_eq!(
            reser.get("raw").and_then(|t| t.as_str()),
            Some(expected_raw),
            "round trip altered the target text for {json}"
        );
    }
}

#[test]
fn expand_after_policy_matches_plan() {
    // Plan policy: Bold/Italic/Code/Strike/Underline/Sub/Super = After;
    // Link/Verbatim = None.
    for k in [
        "bold",
        "italic",
        "code",
        "strike",
        "underline",
        "sub",
        "super",
    ] {
        assert!(InlineMark::expand_after(k), "{k} should expand after");
    }
    for k in ["link", "verbatim"] {
        assert!(!InlineMark::expand_after(k), "{k} should NOT expand after");
    }
}

#[test]
#[should_panic(expected = "start (5) must be <= end (3)")]
fn span_rejects_inverted_range() {
    MarkSpan::new(5, 3, InlineMark::Bold);
}

/// The derived `Deserialize` used to bypass `MarkSpan::new`'s assert, so a
/// persisted `{"start":5,"end":2}` reconstructed silently and only the GPUI
/// renderer's clamp stood between it and a paint abort. Parsing is now the
/// check: an inverted span is an `Err`, and the message names both offsets
/// so the row is identifiable from the log alone.
#[test]
fn deserialize_rejects_an_inverted_span() {
    let e = serde_json::from_str::<MarkSpan>(r#"{"start":5,"end":2,"kind":"Bold"}"#)
        .expect_err("an inverted span must not deserialize");
    let msg = e.to_string();
    assert!(
        msg.contains("start (5)") && msg.contains("end (2)"),
        "the error must name both offsets, got {msg:?}"
    );
}

/// The whole-list boundary, which is what storage actually calls: one bad
/// span fails the parse rather than being healed into the set.
#[test]
fn marks_from_json_rejects_an_inverted_span() {
    let e = marks_from_json(
        r#"[{"start":0,"end":2,"kind":"Bold"},{"start":9,"end":4,"kind":"Italic"}]"#,
    )
    .expect_err("an inverted span must fail the list parse");
    assert!(
        e.to_string().contains("start (9)"),
        "the error must name the offending span, got {e}"
    );
}

/// The discriminating control: `end` past the content is NOT a parse error.
/// `MarkSpan` alone cannot see the content, so range-vs-content stays the
/// read boundary's disclosed heal (`canonicalize_marks_against`). If this
/// ever starts failing, the two policies have been conflated.
#[test]
fn deserialize_accepts_an_in_order_span_regardless_of_content_length() {
    let span: MarkSpan =
        serde_json::from_str(r#"{"start":3,"end":9001,"kind":"Bold"}"#).expect("in-order");
    assert_eq!((span.start, span.end), (3, 9001));
}
