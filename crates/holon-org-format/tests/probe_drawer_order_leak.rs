//! `_drawer_order` is internal bookkeeping, not drawer content.
//!
//! The parser records the authored `:PROPERTIES:` key order so the renderer can
//! replay it instead of alphabetizing (task #88). That carrier lives in the
//! block's properties bag, so the ONE thing that must hold is that it never
//! escapes back into the drawer it describes — otherwise write-back would emit
//! a literal `:_drawer_order:` key and corrupt the vault.

use std::path::Path;

use holon_api::EntityUri;
use holon_org_format::OrgBlockExt;
use holon_org_format::org_props::DRAWER_ORDER;

/// Byte-identical to
/// `holon_integration_tests::pbt::composed::wide_e2e::FORWARD_EDGE_ORG` — the
/// keystone's forward-edge corpus, whose `fe-blocked` is the only block with a
/// non-`:ID:` drawer key.
const FORWARD_EDGE_ORG: &str = "#+ID: forward-edge-page\n* fe-parent\n:PROPERTIES:\n:ID: \
                                fe-parent\n:END:\n* fe-blocked\n:PROPERTIES:\n:ID: \
                                fe-blocked\n:REQUIRES: fe-target\n:END:\n* \
                                fe-target\n:PROPERTIES:\n:ID: fe-target\n:END:\n";

#[test]
fn forward_edge_corpus_drawer_order_property() {
    let parsed = holon_org_format::parser::parse_org_file(
        Path::new("forward-edge-page.org"),
        FORWARD_EDGE_ORG,
        &EntityUri::no_parent(),
        Path::new(""),
    )
    .expect("the keystone's forward-edge corpus must parse");

    let blocked = parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "fe-blocked")
        .expect("fe-blocked must be parsed");

    // The carrier is present: `:REQUIRES:` is the one authored non-`:ID:` key.
    assert_eq!(
        blocked
            .get_property(DRAWER_ORDER)
            .and_then(|v| v.as_string().map(|s| s.to_string()))
            .as_deref(),
        Some(r#"["REQUIRES"]"#),
        "the parser must record fe-blocked's authored drawer key order"
    );

    // …and it is invisible to the drawer serializer, for EVERY parsed block.
    for b in &parsed.blocks {
        assert!(
            !b.drawer_properties().contains_key(DRAWER_ORDER),
            "block {} leaked {DRAWER_ORDER} into its :PROPERTIES: drawer",
            b.id
        );
    }
}
