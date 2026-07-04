//! A FILE-LEVEL `:PROPERTIES:` drawer — the standard Emacs org form since 9.0
//! and org-roam's default identity carrier — must survive the org round trip.
//!
//! Shape (the drawer is the first element in the file; org-mode and orgize both
//! only recognise it there):
//!
//! ```org
//! :PROPERTIES:
//! :ID: 20260807T101010
//! :ROAM_REFS: https://example.com/paper
//! :END:
//! #+TITLE: Preserved
//! * First heading
//! ```
//!
//! Two properties are under test:
//!
//! 1. **Byte-identical round trip** — `parse → render` returns the file
//!    unchanged. The drawer is authored data; write-back may never delete it.
//! 2. **Identity mapping** — the drawer's `:ID:` IS the document identity, the
//!    same role `docs/Reference/ORG_SYNTAX.md` gives `#+ID:`.
//!
//! A CONTROL fixture with no drawer pins that the round trip is byte-stable
//! for this file shape already, so a red here is attributable to the drawer
//! alone and not to unrelated render normalization.

use std::path::Path;

use holon_api::EntityUri;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const ROOT: &str = "/vault";
const FILE: &str = "/vault/page.org";

/// Parse `org` and render the parsed document straight back out.
fn reemit(org: &str) -> String {
    let parsed = parse_org_file(
        Path::new(FILE),
        org,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("fixture parses");
    OrgRenderer::render_document(
        &parsed.document,
        &parsed.blocks,
        Path::new(FILE),
        &parsed.document.id,
    )
}

/// CONTROL: the same file shape carrying its identity as `#+ID:` is already a
/// byte-stable round trip. Any red in the drawer tests below is therefore the
/// drawer's doing.
#[test]
fn control_hash_id_file_round_trips_byte_identically() {
    let org = "#+ID: 20260807T101010\n#+TITLE: Preserved\n* First heading\n:PROPERTIES:\n:ID: \
               h1\n:END:\n";
    assert_eq!(org, reemit(org), "control fixture must be byte-stable");
}

/// The drawer must come back byte-for-byte. Today it is silently deleted and a
/// fresh `#+ID:` with a minted UUID takes its place.
#[test]
fn file_level_drawer_round_trips_byte_identically() {
    let org = ":PROPERTIES:\n:ID: 20260807T101010\n:ROAM_REFS: \
               https://example.com/paper\n:CATEGORY: research\n:END:\n#+TITLE: Preserved\n* First \
               heading\n:PROPERTIES:\n:ID: h1\n:END:\n";
    assert_eq!(
        org,
        reemit(org),
        "a hand-authored file-level :PROPERTIES: drawer is authored data — write-back must not \
         delete or rewrite it"
    );
}

/// The drawer's `:ID:` is the document identity, exactly like `#+ID:`. Bare in
/// the file, `block:`-scheme'd at the parse boundary (ORG_SYNTAX.md).
#[test]
fn file_level_drawer_id_is_the_document_identity() {
    let org = ":PROPERTIES:\n:ID: 20260807T101010\n:END:\n#+TITLE: Preserved\n* First heading\n";
    let parsed = parse_org_file(
        Path::new(FILE),
        org,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("fixture parses");
    assert_eq!(
        "block:20260807T101010",
        parsed.document.id.as_str(),
        "the drawer's :ID: must own the document identity, not a freshly minted UUID"
    );
    assert_eq!(
        "block:20260807T101010",
        parsed.blocks[0].parent_id.as_str(),
        "top-level headlines must be parented to the drawer-identified document"
    );
}

/// Keys Holon does not model are still the user's data: preserved verbatim,
/// in the order they were authored.
#[test]
fn non_id_drawer_keys_are_preserved_verbatim() {
    let org = ":PROPERTIES:\n:ID: doc-1\n:ROAM_REFS: https://example.com/paper\n:CATEGORY: \
               research\n:MYSTERY_KEY: some value with spaces\n:END:\n#+TITLE: Preserved\n* h\n:PROPERTIES:\n:ID: h1\n:END:\n";
    let out = reemit(org);
    for expected in [
        ":ROAM_REFS: https://example.com/paper",
        ":CATEGORY: research",
        ":MYSTERY_KEY: some value with spaces",
    ] {
        assert!(
            out.contains(expected),
            "unmodelled drawer key must round-trip verbatim: missing {expected:?} \
             from\n---\n{out}---"
        );
    }
    assert_eq!(org, out, "authored key order must be replayed unchanged");
}

/// A drawer with no `:ID:` keeps its keys AND leaves the `#+ID:` identity
/// carrier alone — the two are independent.
#[test]
fn drawer_without_id_keeps_the_hash_id_identity() {
    let org = ":PROPERTIES:\n:CATEGORY: research\n:END:\n#+ID: 20260807T101010\n#+TITLE: \
               Preserved\n* h\n:PROPERTIES:\n:ID: h1\n:END:\n";
    let parsed = parse_org_file(
        Path::new(FILE),
        org,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("fixture parses");
    assert_eq!("block:20260807T101010", parsed.document.id.as_str());
    assert_eq!(org, reemit(org));
}

/// `#+ID:` and a drawer `:ID:` naming the SAME id is not a conflict — the file
/// simply states its identity twice. Both carriers are kept.
#[test]
fn agreeing_hash_id_and_drawer_id_is_accepted() {
    let org = ":PROPERTIES:\n:ID: same-id\n:END:\n#+ID: same-id\n#+TITLE: Preserved\n* h\n:PROPERTIES:\n:ID: h1\n:END:\n";
    let parsed = parse_org_file(
        Path::new(FILE),
        org,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("agreeing identity carriers must parse");
    assert_eq!("block:same-id", parsed.document.id.as_str());
    assert_eq!(org, reemit(org));
}

/// A REAL org-roam file is padded (`org-property-format`, `%-10s`), and this
/// feature exists for real org-roam files — so the honest claim has to be
/// stated as what it is: NOT byte-identical on the first pass, a fixed point
/// from the second. Every key and value survives; the alignment does not.
#[test]
fn emacs_padding_converges_to_the_canonical_form_in_one_pass() {
    let padded = ":PROPERTIES:\n:ID:       20260807T101010\n:ROAM_REFS: \
                  https://example.com/paper\n:END:\n#+TITLE: Preserved\n* h\n:PROPERTIES:\n:ID: \
                  h1\n:END:\n";
    let canonical = ":PROPERTIES:\n:ID: 20260807T101010\n:ROAM_REFS: \
                     https://example.com/paper\n:END:\n#+TITLE: Preserved\n* h\n:PROPERTIES:\n:ID: \
                     h1\n:END:\n";

    let once = reemit(padded);
    assert_ne!(
        padded, once,
        "this test exists to PIN that padding is rewritten — if it ever round-trips \
         byte-identically, the docs' disclosure is now wrong and should be corrected"
    );
    assert_eq!(
        canonical, once,
        "padding converges to the single-space form"
    );
    assert_eq!(
        canonical,
        reemit(&once),
        "and the second pass is a fixed point"
    );
}

/// An empty `:ID:` names nothing. Filling it in would write the document's own
/// id — for a name-chain document, its FILE PATH — into the drawer as if the
/// author had put it there.
#[test]
fn an_empty_drawer_id_is_not_an_identity_and_is_never_filled_in() {
    let org = ":PROPERTIES:\n:ID: \n:CATEGORY: research\n:END:\n#+TITLE: Preserved\n* \
               h\n:PROPERTIES:\n:ID: h1\n:END:\n";
    let parsed = parse_org_file(
        Path::new(FILE),
        org,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("an empty :ID: is not a parse error");
    assert_eq!(
        "file:page.org",
        parsed.document.id.as_str(),
        "an empty :ID: must not be treated as the identity carrier"
    );
    let out = reemit(org);
    assert!(
        !out.contains("page.org"),
        "the document's own path must never be written into the drawer; got:\n---\n{out}---"
    );
    assert_eq!(org, out, "the empty value is preserved as authored");
}

/// LIVE COUNTEREXAMPLE 1. Org's `drawer_begin_node` opens with `space0`, so an
/// indented `:PROPERTIES:` is still a file-level drawer. When only the cheap
/// identity probe missed that, the author's `:ID:` was rewritten in place with
/// a minted uuid and nothing was logged.
#[test]
fn an_indented_file_level_drawer_still_identifies_the_document() {
    let org = "  :PROPERTIES:\n  :ID: 20260807T101010\n  :ROAM_REFS: \
               https://example.com/paper\n  :END:\n#+TITLE: Preserved\n* h\n:PROPERTIES:\n:ID: \
               h1\n:END:\n";
    let parsed = parse_org_file(
        Path::new(FILE),
        org,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("an indented file-level drawer parses");
    assert_eq!(
        "block:20260807T101010",
        parsed.document.id.as_str(),
        "indentation does not stop a drawer from being the file's drawer"
    );

    // Indentation is CANONICALIZED to column 0, like Emacs' value padding.
    // Disclosed in ORG_SYNTAX.md; every key and value survives.
    let out = reemit(org);
    assert!(
        out.starts_with(":PROPERTIES:\n:ID: 20260807T101010\n"),
        "got:\n---\n{out}---"
    );
    assert!(
        out.contains(":ROAM_REFS: https://example.com/paper\n"),
        "got:\n---\n{out}---"
    );
    assert_eq!(out, reemit(&out), "and the canonical form is a fixed point");
}

/// LIVE COUNTEREXAMPLE 2. orgize's `node_property_node` requires whitespace
/// after the key, so a value-less `:KEY:` made its WHOLE drawer fail to parse —
/// the file went identity-less, got `#+ID:` stamped on, and the drawer sank
/// below `#+TITLE:` where it is no longer file-level at all.
#[test]
fn a_value_less_key_does_not_void_the_drawer() {
    let org = ":PROPERTIES:\n:ID: 20260807T101010\n:ARCHIVED:\n:ROAM_REFS: \
               https://example.com/paper\n:END:\n#+TITLE: Preserved\n* h\n:PROPERTIES:\n:ID: \
               h1\n:END:\n";
    let parsed = parse_org_file(
        Path::new(FILE),
        org,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("a value-less drawer key parses");
    assert_eq!(
        "block:20260807T101010",
        parsed.document.id.as_str(),
        "one value-less key must not cost the document its identity"
    );

    let out = reemit(org);
    assert!(
        out.starts_with(":PROPERTIES:\n"),
        "the drawer must stay file-level, above #+TITLE:; got:\n---\n{out}---"
    );
    assert!(
        out.contains(":ARCHIVED: \n"),
        "the value-less key is preserved, with the trailing space that lets it re-parse; \
         got:\n---\n{out}---"
    );
    assert!(
        out.contains(":ROAM_REFS: https://example.com/paper\n"),
        "got:\n---\n{out}---"
    );
    assert_eq!(out, reemit(&out), "and the canonical form is a fixed point");
}

/// The drawer is not swallowed when it is not a drawer: a `:PROPERTIES:` block
/// containing a non-property line is left to orgize as ordinary text, rather
/// than Holon's hand-read consuming content the author never put in a drawer.
#[test]
fn a_malformed_opening_block_is_not_taken_as_a_file_drawer() {
    let org = ":PROPERTIES:\nthis is not a property line\n:END:\n#+ID: kw-id\n#+TITLE: \
               Preserved\n* h\n:PROPERTIES:\n:ID: h1\n:END:\n";
    let parsed = parse_org_file(
        Path::new(FILE),
        org,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("parses");
    assert_eq!(
        "block:kw-id",
        parsed.document.id.as_str(),
        "identity still comes from #+ID: — no file drawer was recognized"
    );
}

/// Two DISAGREEING identity carriers in one file is unresolvable: silently
/// picking one discards an authored identity, which is the very failure this
/// lane exists to end. Fail loud at the parse boundary (same policy as the
/// id-collision check next to it).
#[test]
fn conflicting_hash_id_and_drawer_id_is_rejected() {
    let org = ":PROPERTIES:\n:ID: drawer-id\n:END:\n#+ID: keyword-id\n#+TITLE: Preserved\n* h\n:PROPERTIES:\n:ID: h1\n:END:\n";
    let msg = match parse_org_file(
        Path::new(FILE),
        org,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    ) {
        Err(e) => e.to_string(),
        Ok(parsed) => panic!(
            "disagreeing identity carriers must be rejected, never silently resolved — parse \
             succeeded and picked {}",
            parsed.document.id.as_str()
        ),
    };
    for needle in ["drawer-id", "keyword-id", "page.org"] {
        assert!(
            msg.contains(needle),
            "the error must name both ids and the file; got {msg:?}"
        );
    }
}
