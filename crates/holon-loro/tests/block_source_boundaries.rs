//! What a block can carry from one write into the Loro docs: which doc holds
//! it, and which property values the docs can hold.

use std::collections::HashMap;

use anyhow::Result;
use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_api::Value;
use holon_core::cell_registry::EntityCellRegistry;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::block_cell_registry::BlockCellRegistry;
use holon_loro::loro_backend::snapshot_blocks_from_doc;

async fn registry(store: &LoroDocumentStore) -> Result<BlockCellRegistry> {
    Ok(BlockCellRegistry::with_loro(
        store.get_doc(DocScope::Global).await?,
        store.get_doc(DocScope::Layout).await?,
        std::sync::Arc::new(holon_core::NoReadOnlyDocuments),
    ))
}

async fn create(
    registry: &BlockCellRegistry,
    parent: &EntityUri,
    id: &EntityUri,
    properties: &HashMap<String, Value>,
) -> Result<()> {
    registry
        .create_entity(
            parent,
            None,
            id,
            BlockContent::text("x"),
            properties,
            &BlockEdges::default(),
        )
        .await?;
    Ok(())
}

async fn ids_in(store: &LoroDocumentStore, scope: DocScope) -> Result<Vec<String>> {
    let doc = store.get_doc(scope).await?;
    let mut ids: Vec<String> = doc
        .with_read(|d| Ok(snapshot_blocks_from_doc(d)))?
        .into_keys()
        .collect();
    ids.sort();
    Ok(ids)
}

/// A move across the docs is refused, but a delete and a create of the same
/// id is not: the id changes doc between two commits.
#[tokio::test]
async fn an_id_deleted_in_the_global_doc_can_be_created_in_the_layout_doc() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = LoroDocumentStore::new(dir.path().to_path_buf());
    let registry = registry(&store).await?;
    let layout_root = EntityUri::block("__default__");
    let wanderer = EntityUri::block("wanderer");
    create(
        &registry,
        &EntityUri::no_parent(),
        &layout_root,
        &HashMap::new(),
    )
    .await?;
    create(
        &registry,
        &EntityUri::no_parent(),
        &wanderer,
        &HashMap::new(),
    )
    .await?;
    assert_eq!(
        ids_in(&store, DocScope::Global).await?,
        vec!["block:wanderer"]
    );

    assert!(registry.delete_entity(&wanderer).await?);
    create(&registry, &layout_root, &wanderer, &HashMap::new()).await?;

    assert!(ids_in(&store, DocScope::Global).await?.is_empty());
    assert_eq!(
        ids_in(&store, DocScope::Layout).await?,
        vec!["block:__default__", "block:wanderer"]
    );
    Ok(())
}

/// The docs store each property as JSON, which has no NaN or infinity, so a
/// block read from a doc holds only finite floats.
#[tokio::test]
async fn a_non_finite_float_property_reads_back_as_null() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = LoroDocumentStore::new(dir.path().to_path_buf());
    let registry = registry(&store).await?;
    let id = EntityUri::block("floats");
    let properties = HashMap::from([
        ("nan".to_string(), Value::Float(f64::NAN)),
        ("inf".to_string(), Value::Float(f64::INFINITY)),
        ("neg_inf".to_string(), Value::Float(f64::NEG_INFINITY)),
        (
            "nested".to_string(),
            Value::Array(vec![Value::Float(f64::NAN)]),
        ),
    ]);
    create(&registry, &EntityUri::no_parent(), &id, &properties).await?;

    let doc = store.get_doc(DocScope::Global).await?;
    let blocks = doc.with_read(|d| Ok(snapshot_blocks_from_doc(d)))?;
    let stored = &blocks[id.as_str()].block.properties;
    assert_eq!(stored["nan"], Value::Null);
    assert_eq!(stored["inf"], Value::Null);
    assert_eq!(stored["neg_inf"], Value::Null);
    assert_eq!(stored["nested"], Value::Array(vec![Value::Null]));
    Ok(())
}
