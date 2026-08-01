//! Investigation probe (2026-08-01): does the parser persist `_drawer_order`
//! into the block's properties bag for the keystone's forward-edge corpus?

use std::path::Path;

use holon_api::EntityUri;

/// Byte-identical to
/// `holon_integration_tests::pbt::composed::wide_e2e::FORWARD_EDGE_ORG`.
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
    for b in &parsed.blocks {
        println!(
            "PROBE id={} drawer_order={:?} prop_keys={:?} requires={:?}",
            b.id,
            b.get_property("_drawer_order"),
            b.properties.keys().collect::<Vec<_>>(),
            b.requires
        );
    }
    let blocked = parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "fe-blocked")
        .expect("fe-blocked must be parsed");
    assert!(
        blocked.get_property("_drawer_order").is_none(),
        "PROBE RED: parser persisted _drawer_order={:?} into fe-blocked's properties bag",
        blocked.get_property("_drawer_order")
    );
}
