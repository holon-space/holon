//! Shared block-content proptest strategies.
//!
//! @pbt kind generator
//! @pbt gen extended arms (multi-byte, empty, whitespace-only, org-special
//!   prefixes, mid-word underscores) are the ONLY producers of those classes;
//!   every default consumer mixes them at ~4/10 weight EXCEPT the org-special
//!   spelling sub-arm and the empty/whitespace sub-arms, which are 1/N INSIDE
//!   extended_content_arm — so empty content and `:PROPERTIES:`-prefix headlines
//!   arrive at ~4% each even when the extended arm is live
//!
//! Pure proptest generators (no reference-state or SUT types), lifted to the
//! shared floor so both the central keystone generators and the co-located
//! per-subsystem transitions (`holon-loro-testing`'s peer edits) draw the same
//! extended-content distribution — multi-byte runs, org-special prefixes, and
//! mid-word-underscore identifiers that stress the org round-trip.

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

/// A single "extended" character: ASCII, plus 2/3/4-byte codepoints. The
/// multi-byte codepoints are the payload: any path that converts byte offsets
/// to keystroke counts (or slices content at a char boundary it computed in the
/// other unit) breaks on these.
pub fn extended_char() -> BoxedStrategy<char> {
    prop_oneof![
        2 => proptest::char::range('a', 'z'),
        1 => Just(' '),
        2 => prop_oneof![Just('é'), Just('ß'), Just('ñ')],   // 2-byte
        2 => prop_oneof![Just('€'), Just('中'), Just('日')], // 3-byte
        2 => prop_oneof![Just('😀'), Just('🎉')],            // 4-byte emoji
    ]
    .boxed()
}

/// Single-line extended content: multi-byte runs, empty, whitespace-only,
/// and org-special prefixes (`*`, `[[…]]`, `:PROPERTIES:`, `#+`) that a user
/// can legitimately type but the org renderer must round-trip.
pub fn extended_content_arm() -> BoxedStrategy<String> {
    prop_oneof![
        4 => prop::collection::vec(extended_char(), 1..=12)
            .prop_map(|cs| cs.into_iter().collect::<String>()),
        1 => Just(String::new()),
        1 => prop_oneof![Just("   ".to_string()), Just("\t".to_string())],
        // Org-special spellings. `:PROPERTIES:{tail}` (incl. the bare
        // trailing-tag-group form) is BY-DESIGN org tag syntax — the ref
        // model mirrors it via `split_headline_tags` in
        // `normalize_content_for_org_roundtrip`; pinned in
        // holon-org-format/tests/properties_prefix_headline_repro.rs.
        2 => ("[A-Za-z0-9 ]{0,12}", 0..4u8).prop_map(|(tail, kind)| match kind {
            0 => format!("* {tail}"),
            1 => format!("[[{tail}]]"),
            2 => format!("#+ {tail}"),
            _ => format!(":PROPERTIES:{tail}"),
        }),
        // snake_case identifiers with mid-word underscores — the single
        // biggest content class in a code-heavy PKM (`focused_block`,
        // `sort_key`, `keyed_rows_signal_vec`). orgize parses a mid-word `_`
        // (preceded by a word char, so it can't open org UNDERLINE) as a bare
        // SUBSCRIPT, and the mark-extraction round-trip used to destroy the
        // surrounding characters (`focused_block` → `focusedloc`) — a silent
        // vault-corrupting data-loss bug. This arm de-vacuums the org
        // round-trip's inline-mark path (the default ASCII generators never
        // emit `_`). See handoff 2026-07-06 (org round-trip mangles content)
        // + the `bare_underscore_identifiers_survive_round_trip` regression in
        // holon-org-format/src/inline_marks.rs.
        2 => "[a-z]{1,6}(_[a-z]{1,6}){1,4}",
    ]
    .boxed()
}

/// Peer-edit content (`PeerEdit::{Create,Update}`). Extended mode adds
/// multi-byte and org-special content arriving from a remote peer.
pub fn peer_content_strategy() -> BoxedStrategy<String> {
    let base = "[a-z]{4,8}".prop_map(|s| s).boxed();
    prop_oneof![6 => base, 4 => extended_content_arm()].boxed()
}
