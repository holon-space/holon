//! `LoroBlockOrdering::update_in_tree` is the whole org→block write path of a
//! no-Turso session, so an edge field it leaves in the param bag is an
//! authored drawer lost: it is stored as a property, and the Loro read
//! boundary strips edge columns out of properties.
//!
//! @pbt kind harness
//! @pbt covers loro-seam-edge-fields — every `EdgeField` a create/update
//! param bag carries reaches the block's typed junction slot

use std::sync::Arc;

use holon_api::EdgeField;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::repository::CoreOperations;
use holon_app::loro_seams::LoroBlockOrdering;
use holon_core::block_ordering::BlockOrdering;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::loro_document::LoroDocument;

#[tokio::test]
async fn update_in_tree_routes_every_edge_field_to_its_junction() {
    let doc = LoroDocument::new("seam-test".to_string()).expect("loro doc");
    let backend = Arc::new(LoroBackend::from_document(Arc::new(doc)));
    let root = backend.create_placeholder_root("root").await.expect("root");
    let seam = LoroBlockOrdering::new(backend.clone());

    let mut params: StorageEntity = StorageEntity::new();
    params.insert("id".into(), Value::String("block:step".into()));
    params.insert("parent_id".into(), Value::String(root.clone()));
    params.insert("content".into(), Value::String("a step".into()));
    params.insert(
        "tags".into(),
        Value::Array(vec![Value::String("task".into())]),
    );
    params.insert(
        "requires".into(),
        Value::Array(vec![Value::String("block:dep".into())]),
    );
    params.insert(
        "advice_suppressed".into(),
        Value::Array(vec![Value::String("block:lesson".into())]),
    );
    params.insert(
        "contributes_to".into(),
        Value::Array(vec![Value::String("block:goal".into())]),
    );
    seam.update_in_tree(params).await.expect("update_in_tree");

    let block = backend.get_block("block:step").await.expect("read back");
    assert_eq!(block.tags.to_vec(), vec!["task".to_string()]);
    assert_eq!(uris(&block.requires), vec!["block:dep".to_string()]);
    assert_eq!(
        uris(&block.advice_suppressed),
        vec!["block:lesson".to_string()]
    );
    assert_eq!(uris(&block.contributes_to), vec!["block:goal".to_string()]);

    for field in EdgeField::ALL {
        assert!(
            !block.properties.contains_key(field.column()),
            "edge field {} was written into the properties bag: {:?}",
            field.column(),
            block.properties
        );
    }
}

fn uris(targets: &[EntityUri]) -> Vec<String> {
    targets.iter().map(|u| u.to_string()).collect()
}
