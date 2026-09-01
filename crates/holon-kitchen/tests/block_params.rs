//! Inc A rung 3 — the ingest params a `.cook` block actually sends to the
//! store. Parsing a step correctly is worth nothing if the params builder drops
//! what it parsed.

use std::path::Path;
use std::path::PathBuf;

use holon_api::EntityUri;
use holon_api::ROUTING_DOC_URI_KEY;
use holon_api::Value;
use holon_core::file_format::FileFormatAdapter;
use holon_kitchen::CookFormatAdapter;
use holon_kitchen::STEP_NUMBER_KEY;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn parsed() -> holon_core::file_format::FileFormatParseResult {
    let root = fixtures();
    let path = root.join("pancakes.cook");
    let content = std::fs::read_to_string(&path).unwrap();
    CookFormatAdapter::new()
        .parse(&path, &content, &EntityUri::no_parent(), &root)
        .unwrap()
}

#[test]
fn step_number_survives_into_the_ingest_params() {
    let r = parsed();
    let step = r
        .blocks
        .iter()
        .find(|b| b.get_property_str(STEP_NUMBER_KEY).as_deref() == Some("1"))
        .expect("no step 1");

    let params =
        holon_kitchen::params::build_block_params(step, &r.document.id, &r.document.id, None)
            .unwrap();

    assert_eq!(
        params.get(STEP_NUMBER_KEY),
        Some(&Value::String("1".to_string())),
        "step_number was dropped — the step/prose distinction never reaches the store"
    );
}

#[test]
fn document_metadata_survives_into_the_ingest_params() {
    let r = parsed();
    let params = holon_kitchen::params::build_block_params(
        &r.document,
        &EntityUri::no_parent(),
        &r.document.id,
        None,
    )
    .unwrap();

    assert_eq!(
        params.get("servings"),
        Some(&Value::String("4".to_string())),
        "servings dropped from document params"
    );
    assert_eq!(
        params.get("course"),
        Some(&Value::String("breakfast".to_string())),
        "course dropped from document params"
    );
}

#[test]
fn params_carry_identity_content_and_routing() {
    let r = parsed();
    let step = &r.blocks[0];
    let params =
        holon_kitchen::params::build_block_params(step, &r.document.id, &r.document.id, None)
            .unwrap();

    assert_eq!(params.get("id"), Some(&Value::String(step.id.to_string())));
    assert_eq!(
        params.get(ROUTING_DOC_URI_KEY),
        Some(&Value::String(r.document.id.to_string())),
        "routing key missing — the op cannot reach its owning document"
    );
    assert_eq!(
        params.get("content"),
        Some(&Value::String(step.content.clone()))
    );
}

#[test]
fn a_property_naming_a_storage_column_is_refused_not_silently_inserted() {
    // `build_block_params` is pub, so a caller that never went through the
    // parse boundary can reach it. A debug_assert would be a no-op in release
    // — exactly where emitting `content` as a param would overwrite the row's
    // own text unseen.
    let r = parsed();
    let mut block = r.blocks[0].clone();
    block.set_property("content".to_string(), "hijacked");

    let msg =
        holon_kitchen::params::build_block_params(&block, &r.document.id, &r.document.id, None)
            .err()
            .expect("a storage-column property key must be refused")
            .to_string();
    assert!(
        msg.contains("content") && msg.contains("storage column"),
        "refusal must name the key and why: {msg}"
    );
}
