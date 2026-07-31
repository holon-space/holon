//! A `:PROPERTIES:` drawer's KEY ORDER is authored data: the writer chose it,
//! and a write-back that reorders it churns the file on disk for no semantic
//! gain. The one file the vault byte-stability simulation could not keep
//! stable was exactly this — five custom keys authored in a non-alphabetical
//! order, re-emitted sorted.
//!
//! Property under test: `parse_org_file → render_document` is the identity on
//! a file whose drawer keys are deliberately NOT alphabetical.

use std::path::Path;

use holon_api::EntityUri;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const ROOT: &str = "/test";
const FILE: &str = "/test/page.org";

/// One write-back pass: parse, render the parsed blocks back, return the bytes.
fn write_back(source: &str) -> String {
    let parsed = parse_org_file(
        Path::new(FILE),
        source,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("parse the fixture");
    OrgRenderer::render_document(
        &parsed.document,
        &parsed.blocks,
        Path::new(FILE),
        &parsed.document.id,
    )
}

fn assert_stable(source: &str) {
    let rendered = write_back(source);
    assert_eq!(
        source, rendered,
        "write-back moved bytes.\n--- disk ---\n{source}\n--- written ---\n{rendered}"
    );
}

/// The shape of the one file the vault simulation allowlisted: custom drawer
/// keys in authored (reverse-alphabetical-ish) order.
#[test]
fn unordered_custom_drawer_keys_survive_write_back() {
    assert_stable(
        "#+ID: doc-root\n\
         * ENDED STX.BROWSER_AGENT — agent session ended\n\
         :PROPERTIES:\n\
         :ID: 8762126a-82cc-884f-8d95-cdf05c770149\n\
         :STATE: ended\n\
         :PROJECT: STX.BROWSER_AGENT\n\
         :SOURCE: citrix\n\
         :UPDATED-AT: 2026-07-30T09:45:45Z\n\
         :NOTE: agent session ended\n\
         :END:\n\
         agent session ended\n",
    );
}

/// Strictly descending keys — the maximal distance from the sorted order, so a
/// renderer that sorts cannot accidentally pass.
#[test]
fn descending_drawer_keys_survive_write_back() {
    assert_stable(
        "#+ID: doc-root\n\
         * Task\n\
         :PROPERTIES:\n\
         :ID: b1\n\
         :ZULU: 1\n\
         :YANKEE: 2\n\
         :XRAY: 3\n\
         :WHISKEY: 4\n\
         :END:\n",
    );
}

/// `:ID:` is authored LAST here but is canonically emitted FIRST — the one
/// deliberate reordering. The remaining keys keep their authored order.
#[test]
fn id_is_hoisted_first_and_the_rest_keep_authored_order() {
    let rendered = write_back(
        "#+ID: doc-root\n\
         * Task\n\
         :PROPERTIES:\n\
         :ZULU: 1\n\
         :ALPHA: 2\n\
         :ID: b1\n\
         :END:\n",
    );
    assert_eq!(
        rendered,
        "#+ID: doc-root\n\
         * Task\n\
         :PROPERTIES:\n\
         :ID: b1\n\
         :ZULU: 1\n\
         :ALPHA: 2\n\
         :END:\n",
    );
}

/// Two passes must reach the same bytes as one — write-back is idempotent, so
/// a vault does not drift key-by-key across successive syncs.
#[test]
fn write_back_is_idempotent_on_unordered_keys() {
    let source = "#+ID: doc-root\n\
                  * Task\n\
                  :PROPERTIES:\n\
                  :ID: b1\n\
                  :STATE: ended\n\
                  :PROJECT: p\n\
                  :SOURCE: citrix\n\
                  :END:\n";
    let once = write_back(source);
    let twice = write_back(&once);
    assert_eq!(
        once, twice,
        "second write-back moved bytes the first did not"
    );
}

/// Two keys differing ONLY in case are two distinct drawer keys, and a real
/// vault headline carries exactly that (`:Effort:` and `:effort:` side by side,
/// straddling a `:REQUIRES:`). Collapsing them case-insensitively hands one the
/// other's slot and displaces whatever sat between them.
#[test]
fn keys_differing_only_in_case_keep_separate_slots() {
    assert_stable(
        "#+ID: doc-root\n\
         * TODO Task\n\
         :PROPERTIES:\n\
         :ID: b1\n\
         :Effort: 1:00\n\
         :REQUIRES: edge-field-descriptor\n\
         :effort: 1:00\n\
         :gate: G1\n\
         :END:\n",
    );
}

/// Drawer keys the parser LIFTS onto typed block fields (`:REQUIRES:`,
/// `:COLLAPSED:`) are re-synthesized on render rather than carried through the
/// authored bucket. They must still land in their authored slot, not appended.
#[test]
fn lifted_keys_keep_their_authored_slot() {
    assert_stable(
        "#+ID: doc-root\n\
         * TODO Task\n\
         :PROPERTIES:\n\
         :ID: b1\n\
         :STATE: ended\n\
         :REQUIRES: dep-a dep-b\n\
         :SOURCE: citrix\n\
         :COLLAPSED: t\n\
         :END:\n",
    );
}
