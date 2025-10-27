// Standalone test of serialize_blocks_to_org for multi-line content
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_integration_tests::serialize_blocks_to_org_with_doc;

#[test]
fn multi_line_content_puts_body_after_properties() {
    let doc = EntityUri::block("doc_id");
    let block = Block::new_text(
        EntityUri::block("abc"),
        doc.clone(),
        "title line\nbody one\nbody two",
    );

    let out = serialize_blocks_to_org_with_doc(&[&block], &doc, None);
    println!("---OUTPUT---\n{out}---END---");

    // The :PROPERTIES: drawer must come BEFORE the body lines
    let prop_pos = out.find(":PROPERTIES:").expect("no drawer");
    let body_pos = out.find("body one").expect("no body");
    assert!(prop_pos < body_pos, ":PROPERTIES: must come before body");
}
