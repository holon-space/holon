//! The create-idempotence guard must ask "is this id live in ANY doc this
//! backend routes over", not "is it live in the GLOBAL tree".
//!
//! The bundled layout lives in the device-local layout doc, so the global-only
//! question answers "no" for every layout id: the next seed pass re-mints the
//! whole subtree and the vault ends up with two live Loro nodes under one
//! stable id — the D77 collision, on every boot.

use std::collections::HashMap;

use anyhow::Result;
use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_core::cell_registry::EntityCellRegistry;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::block_cell_registry::BlockCellRegistry;

/// Live Loro nodes per stable id across BOTH of the store's docs. A count above
/// one is the defect; counting one doc alone would let a duplicate hide in the
/// other.
async fn live_node_counts(store: &LoroDocumentStore) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for scope in [DocScope::Global, DocScope::Layout] {
        let doc = store
            .get_doc(scope)
            .await
            .unwrap_or_else(|e| panic!("the store has no {scope:?} doc: {e:#}"));
        let index = doc
            .with_read(|d| Ok(holon_loro::build_tid_index(d)))
            .unwrap_or_else(|e| panic!("reading the {scope:?} doc failed: {e:#}"));
        for id in index.into_values() {
            *counts.entry(id).or_default() += 1;
        }
    }
    counts
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

/// One pass of the boot seed over a two-block layout: the root (routed to the
/// layout doc by its own id) and a child (routed by its parent). Both arms of
/// `resolve_write_target_for_parent`'s layout routing.
async fn seed_layout(registry: &BlockCellRegistry) -> Result<()> {
    let root = EntityUri::block("__default__");
    let child = EntityUri::block("root-layout");
    registry
        .create_entity(
            &EntityUri::no_parent(),
            None,
            &root,
            BlockContent::text(""),
            &HashMap::new(),
            &BlockEdges::default(),
        )
        .await?;
    registry
        .create_entity(
            &root,
            None,
            &child,
            BlockContent::text("Root Layout"),
            &HashMap::new(),
            &BlockEdges::default(),
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn re_seeding_an_already_migrated_layout_mints_no_second_live_node() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = LoroDocumentStore::new(dir.path().to_path_buf());
    let registry = BlockCellRegistry::with_loro(
        store.get_doc(DocScope::Global).await?,
        store.get_doc(DocScope::Layout).await?,
    );

    seed_layout(&registry).await?;

    // The premise: the layout really is in the layout doc, so the global-only
    // question a re-seed could ask has nothing to find.
    assert_eq!(
        ids_in(&store, DocScope::Layout).await,
        vec![
            "block:__default__".to_string(),
            "block:root-layout".to_string()
        ],
        "the seed must route the layout into the LAYOUT doc — otherwise this test \
         cannot observe the guard at all"
    );
    assert!(
        ids_in(&store, DocScope::Global).await.is_empty(),
        "no layout block belongs in the replicated global doc"
    );

    seed_layout(&registry).await?;

    let counts = live_node_counts(&store).await;
    let doubled: Vec<(&String, &usize)> = counts.iter().filter(|(_, n)| **n > 1).collect();
    assert!(
        doubled.is_empty(),
        "the second seed pass minted a second live Loro node for {doubled:?} — the \
         create-idempotence guard asked the global-only question and missed the \
         layout doc"
    );
    assert_eq!(
        counts.len(),
        2,
        "both layout ids must still be live exactly once: {counts:?}"
    );
    Ok(())
}
