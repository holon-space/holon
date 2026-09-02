//! Boot-blocking behaviour of `layout_migration::migrate_layout_out_of_global`.
//!
//! `LoroModule` calls it on every boot and `expect`s it, so each arm here is on
//! the path between a vault written before the layout doc existed and a usable
//! session.

use std::collections::HashMap;

use anyhow::Result;
use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_core::cell_registry::EntityCellRegistry;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::block_cell_registry::BlockCellRegistry;
use holon_loro::layout_migration::migrate_layout_out_of_global;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::loro_backend::NewBlockWithProperties;

fn new_block(parent: EntityUri, id: &str, content: &str) -> NewBlockWithProperties {
    NewBlockWithProperties {
        parent_id: parent,
        id: EntityUri::block(id),
        content: BlockContent::text(content),
        properties: HashMap::new(),
        edges: BlockEdges::default(),
    }
}

/// Write `blocks` straight into one doc, bypassing the routing a two-doc
/// backend would apply — that is how a pre-split vault's global tree looks.
async fn write_into(
    store: &LoroDocumentStore,
    scope: DocScope,
    blocks: Vec<NewBlockWithProperties>,
) -> Result<()> {
    let backend = LoroBackend::from_document(store.get_doc(scope).await?);
    backend
        .create_blocks_with_properties(blocks)
        .await
        .map_err(|e| anyhow::anyhow!("seeding the {scope:?} doc: {e:?}"))?;
    Ok(())
}

async fn ids_in(store: &LoroDocumentStore, scope: DocScope) -> Vec<String> {
    let doc = store.get_doc(scope).await.expect("doc");
    let mut ids: Vec<String> = doc
        .with_read(|d| Ok(holon_loro::build_tid_index(d)))
        .expect("read")
        .into_values()
        .collect();
    ids.sort();
    ids
}

/// A legacy vault: layout closure and unrelated notes share the global tree.
async fn legacy_store(dir: &tempfile::TempDir) -> Result<LoroDocumentStore> {
    let store = LoroDocumentStore::new(dir.path().to_path_buf());
    let root = EntityUri::block("__default__");
    write_into(
        &store,
        DocScope::Global,
        vec![
            new_block(EntityUri::no_parent(), "__default__", ""),
            new_block(root.clone(), "root-layout", "Root Layout"),
            new_block(EntityUri::block("root-layout"), "sidebar", "Sidebar"),
            new_block(EntityUri::no_parent(), "a-note", "a replicated note"),
        ],
    )
    .await?;
    Ok(store)
}

/// The count this returns is what `LoroModule` logs at INFO when it is above
/// zero, and the second call is the one every subsequent boot makes.
#[tokio::test]
async fn a_legacy_vault_moves_its_layout_closure_once_and_then_moves_nothing() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = legacy_store(&dir).await?;

    let moved = migrate_layout_out_of_global(&store).await?;
    assert_eq!(moved, 3, "the `block:__default__` closure is three blocks");
    assert_eq!(
        ids_in(&store, DocScope::Layout).await,
        vec![
            "block:__default__".to_string(),
            "block:root-layout".to_string(),
            "block:sidebar".to_string(),
        ]
    );
    assert_eq!(
        ids_in(&store, DocScope::Global).await,
        vec!["block:a-note".to_string()],
        "only the layout closure moves — replicated content stays"
    );

    let again = migrate_layout_out_of_global(&store).await?;
    assert_eq!(
        again, 0,
        "the second boot finds nothing left in the global doc"
    );
    assert_eq!(ids_in(&store, DocScope::Layout).await.len(), 3);
    assert_eq!(ids_in(&store, DocScope::Global).await.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_half_migrated_vault_refuses_to_migrate_and_names_the_straddling_ids() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = legacy_store(&dir).await?;
    write_into(
        &store,
        DocScope::Layout,
        vec![new_block(EntityUri::no_parent(), "__default__", "")],
    )
    .await?;

    let err = migrate_layout_out_of_global(&store)
        .await
        .expect_err("an id live in BOTH docs has no rule-decidable resolution");
    let msg = format!("{err:#}");
    assert!(msg.contains("live in BOTH"), "msg = {msg}");
    assert!(msg.contains("block:__default__"), "msg = {msg}");
    assert_eq!(
        ids_in(&store, DocScope::Global).await.len(),
        4,
        "the refusal must not have moved or deleted anything: {msg}"
    );
    Ok(())
}

#[tokio::test]
async fn a_vault_with_no_layout_migrates_nothing_and_seeds_into_the_layout_doc() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = LoroDocumentStore::new(dir.path().to_path_buf());
    write_into(
        &store,
        DocScope::Global,
        vec![new_block(
            EntityUri::no_parent(),
            "a-note",
            "a replicated note",
        )],
    )
    .await?;

    assert_eq!(migrate_layout_out_of_global(&store).await?, 0);
    assert!(ids_in(&store, DocScope::Layout).await.is_empty());

    let registry = BlockCellRegistry::with_loro(
        store.get_doc(DocScope::Global).await?,
        store.get_doc(DocScope::Layout).await?,
    );
    registry
        .create_entity(
            &EntityUri::no_parent(),
            None,
            &EntityUri::block("__default__"),
            BlockContent::text(""),
            &HashMap::new(),
            &BlockEdges::default(),
        )
        .await?;

    assert_eq!(
        ids_in(&store, DocScope::Layout).await,
        vec!["block:__default__".to_string()],
        "a fresh vault's layout is seeded straight into the layout doc"
    );
    assert_eq!(
        ids_in(&store, DocScope::Global).await,
        vec!["block:a-note".to_string()],
        "and never touches the replicated doc"
    );
    Ok(())
}
