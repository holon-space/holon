//! Two peers created the same stable id, so two live nodes of one doc carry
//! it. Every resolver of that doc must name the same carrier, the smallest
//! `TreeID`, whatever order the tree lists the two in.

use std::sync::Arc;

use holon_api::BlockContent;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::repository::CoreOperations;
use holon_core::cell_registry::EntityCellRegistry;
use holon_loro::CONTENT_RAW;
use holon_loro::LoroDocument;
use holon_loro::TREE_NAME;
use holon_loro::WriteOrigin;
use holon_loro::block_cell_registry::BlockCellRegistry;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::shared_tree::InMemorySharedTreeStore;
use holon_loro::snapshot_blocks_from_doc;
use loro::ExportMode;
use loro::TreeID;

const ID: &str = "dup";

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn new_doc(peer: u64) -> Arc<LoroDocument> {
    Arc::new(LoroDocument::new_with_peer_id(format!("dup-{peer}"), Some(peer)).unwrap())
}

fn create_root(rt: &tokio::runtime::Runtime, doc: &Arc<LoroDocument>, text: &str) -> TreeID {
    let backend = LoroBackend::from_document(doc.clone());
    rt.block_on(backend.create_block(
        EntityUri::no_parent(),
        BlockContent::text(text),
        Some(EntityUri::block(ID)),
    ))
    .unwrap();
    doc.find_by_stable_id(ID).unwrap().unwrap()
}

/// A doc where peer 50 and peer 5 each created `ID`: returns the doc, peer
/// 50's carrier and peer 5's (the smaller `TreeID`). `small_first` puts peer
/// 5's carrier first among the roots.
fn duplicated(
    rt: &tokio::runtime::Runtime,
    small_first: bool,
) -> (Arc<LoroDocument>, TreeID, TreeID) {
    let doc = new_doc(50);
    let large = create_root(rt, &doc, "from-50");
    let other = new_doc(5);
    let small = create_root(rt, &other, "from-5");
    let since = doc.with_read(|d| Ok(d.oplog_vv())).unwrap();
    let update = other
        .with_read(|d| Ok(d.export(ExportMode::updates(&since))?))
        .unwrap();
    doc.apply_update(&update).unwrap();
    assert!(small < large, "peer 5's carrier has the smaller TreeID");
    doc.with_write(WriteOrigin::BlockOps, |d| {
        let tree = d.get_tree(TREE_NAME);
        if small_first {
            tree.mov_after(large, small)?;
        } else {
            tree.mov_after(small, large)?;
        }
        Ok(())
    })
    .unwrap();
    (doc, large, small)
}

fn content(doc: &LoroDocument, node: TreeID) -> String {
    doc.with_read(|d| {
        let meta = d.get_tree(TREE_NAME).get_meta(node)?;
        Ok(match meta.get(CONTENT_RAW) {
            Some(loro::ValueOrContainer::Container(loro::Container::Text(text))) => {
                text.to_string()
            }
            other => panic!("{node:?} has no text content: {other:?}"),
        })
    })
    .unwrap()
}

fn deleted(doc: &LoroDocument, node: TreeID) -> bool {
    doc.with_read(|d| Ok(d.get_tree(TREE_NAME).is_node_deleted(&node)?))
        .unwrap()
}

fn kid(doc: &LoroDocument) -> TreeID {
    doc.find_by_stable_id("kid")
        .unwrap()
        .expect("kid was created")
}

fn parent(doc: &LoroDocument, node: TreeID) -> Option<loro::TreeParentId> {
    doc.with_read(|d| Ok(d.get_tree(TREE_NAME).parent(node)))
        .unwrap()
}

fn layout_case(small_first: bool) {
    let rt = rt();
    let (layout, large, small) = duplicated(&rt, small_first);
    let backend = LoroBackend::from_document(new_doc(1)).with_layout_doc(layout.clone());

    rt.block_on(backend.create_block(
        EntityUri::block(ID),
        BlockContent::text("k"),
        Some(EntityUri::block("kid")),
    ))
    .unwrap();
    let kid = kid(&layout);
    assert_eq!(
        parent(&layout, kid),
        Some(loro::TreeParentId::Node(small)),
        "a create under `{ID}` lands under the smallest carrier"
    );

    rt.block_on(backend.update_block(&format!("block:{ID}"), BlockContent::text("edited")))
        .unwrap();
    assert_eq!(
        (content(&layout, small), content(&layout, large)),
        ("edited".to_string(), "from-50".to_string()),
        "an update of `{ID}` edits the smallest carrier, the one the create used"
    );

    rt.block_on(backend.delete_block(&format!("block:{ID}")))
        .unwrap();
    assert!(
        deleted(&layout, small) && deleted(&layout, kid) && !deleted(&layout, large),
        "a delete of `{ID}` removes the smallest carrier and its child"
    );
}

#[test]
fn layout_writes_address_the_smallest_carrier_listed_first() {
    layout_case(true);
}

#[test]
fn layout_writes_address_the_smallest_carrier_listed_last() {
    layout_case(false);
}

fn snapshot_case(small_first: bool) {
    let rt = rt();
    let (doc, _, small) = duplicated(&rt, small_first);
    let backend = LoroBackend::from_document(doc.clone());
    rt.block_on(backend.update_block(&format!("block:{ID}"), BlockContent::text("edited")))
        .unwrap();
    assert_eq!(content(&doc, small), "edited");
    let projected = doc
        .with_read(|d| Ok(snapshot_blocks_from_doc(d)))
        .unwrap()
        .remove(&format!("block:{ID}"))
        .expect("the snapshot projects the id");
    assert_eq!(
        projected.block.content, "edited",
        "the snapshot projects the carrier the update wrote"
    );
}

#[test]
fn the_snapshot_projects_the_smallest_carrier_listed_first() {
    snapshot_case(true);
}

#[test]
fn the_snapshot_projects_the_smallest_carrier_listed_last() {
    snapshot_case(false);
}

fn shared_case(small_first: bool) {
    let rt = rt();
    let (shared, large, small) = duplicated(&rt, small_first);
    let mut store = InMemorySharedTreeStore::new();
    store.insert_arc("share-1".into(), shared.doc());
    let backend = LoroBackend::from_document(new_doc(1)).with_shared_trees(Arc::new(store));

    rt.block_on(backend.update_block(&format!("block:{ID}"), BlockContent::text("edited")))
        .unwrap();
    assert_eq!(
        (content(&shared, small), content(&shared, large)),
        ("edited".to_string(), "from-50".to_string()),
        "an update of a shared `{ID}` edits the smallest carrier"
    );
}

#[test]
fn shared_doc_writes_address_the_smallest_carrier_listed_first() {
    shared_case(true);
}

#[test]
fn shared_doc_writes_address_the_smallest_carrier_listed_last() {
    shared_case(false);
}

fn cell_case(small_first: bool) {
    let rt = rt();
    let (doc, large, small) = duplicated(&rt, small_first);
    let registry = BlockCellRegistry::with_loro_doc(doc.doc());
    let written = rt
        .block_on(registry.write_field(
            &EntityUri::block(ID),
            "content",
            Value::String("edited".into()),
        ))
        .unwrap();
    assert!(written, "the content write goes through the Loro cell");
    assert_eq!(
        (content(&doc, small), content(&doc, large)),
        ("edited".to_string(), "from-50".to_string()),
        "a cell write of `{ID}` edits the smallest carrier"
    );
}

#[test]
fn cell_writes_address_the_smallest_carrier_listed_first() {
    cell_case(true);
}

#[test]
fn cell_writes_address_the_smallest_carrier_listed_last() {
    cell_case(false);
}
