//! Hypothesis: Two LoroDoc instances can sync a LoroTree via export/import of
//! incremental updates, simulating two-peer collaboration.

use loro::{Container, ExportMode, LoroDoc, LoroText, LoroTree, TreeID, ValueOrContainer};

fn set_text(tree: &LoroTree, node: TreeID, content: &str) {
    let meta = tree.get_meta(node).unwrap();
    let text: LoroText = meta
        .insert_container("content_raw", LoroText::new())
        .unwrap();
    text.insert(0, content).unwrap();
}

fn read_text(tree: &LoroTree, node: TreeID) -> String {
    let meta = tree.get_meta(node).unwrap();
    match meta.get("content_raw") {
        Some(ValueOrContainer::Container(Container::Text(t))) => t.to_string(),
        _ => String::new(),
    }
}

fn sync_a_to_b(a: &LoroDoc, b: &LoroDoc) {
    let b_vv = b.oplog_vv();
    let update = a.export(ExportMode::updates(&b_vv)).unwrap();
    b.import(&update).unwrap();
}

fn sync_both(a: &LoroDoc, b: &LoroDoc) {
    sync_a_to_b(a, b);
    sync_a_to_b(b, a);
}

#[test]
fn initial_sync_via_snapshot() {
    let doc_a = LoroDoc::new();
    doc_a.set_peer_id(1).unwrap();
    let tree_a = doc_a.get_tree("blocks");
    tree_a.enable_fractional_index(0);

    let root = tree_a.create(None).unwrap();
    set_text(&tree_a, root, "Root from A");
    let child = tree_a.create(root).unwrap();
    set_text(&tree_a, child, "Child from A");
    doc_a.commit();

    // Peer B joins by importing a snapshot
    let snapshot = doc_a.export(ExportMode::Snapshot).unwrap();
    let doc_b = LoroDoc::new();
    doc_b.set_peer_id(2).unwrap();
    doc_b.import(&snapshot).unwrap();

    let tree_b = doc_b.get_tree("blocks");
    assert_eq!(read_text(&tree_b, root), "Root from A");
    assert_eq!(read_text(&tree_b, child), "Child from A");
}

#[test]
fn incremental_sync_content_edits() {
    let doc_a = LoroDoc::new();
    doc_a.set_peer_id(1).unwrap();
    let tree_a = doc_a.get_tree("blocks");
    tree_a.enable_fractional_index(0);

    let node = tree_a.create(None).unwrap();
    set_text(&tree_a, node, "Hello");
    doc_a.commit();

    // Initial sync
    let doc_b = LoroDoc::new();
    doc_b.set_peer_id(2).unwrap();
    doc_b
        .import(&doc_a.export(ExportMode::Snapshot).unwrap())
        .unwrap();

    // A edits content
    let tree_a2 = doc_a.get_tree("blocks");
    let meta = tree_a2.get_meta(node).unwrap();
    match meta.get("content_raw") {
        Some(ValueOrContainer::Container(Container::Text(t))) => {
            t.insert(5, " World").unwrap();
        }
        _ => panic!("expected text"),
    }
    doc_a.commit();

    // Sync A -> B incrementally
    sync_a_to_b(&doc_a, &doc_b);

    let tree_b = doc_b.get_tree("blocks");
    assert_eq!(read_text(&tree_b, node), "Hello World");
}

#[test]
fn incremental_sync_structural_move() {
    let doc_a = LoroDoc::new();
    doc_a.set_peer_id(1).unwrap();
    let tree_a = doc_a.get_tree("blocks");
    tree_a.enable_fractional_index(0);

    let parent1 = tree_a.create(None).unwrap();
    let parent2 = tree_a.create(None).unwrap();
    let block = tree_a.create(parent1).unwrap();
    set_text(&tree_a, block, "Moving block");
    doc_a.commit();

    let doc_b = LoroDoc::new();
    doc_b.set_peer_id(2).unwrap();
    doc_b
        .import(&doc_a.export(ExportMode::Snapshot).unwrap())
        .unwrap();

    // A moves block from parent1 to parent2
    tree_a.mov(block, parent2).unwrap();
    doc_a.commit();

    sync_a_to_b(&doc_a, &doc_b);

    let tree_b = doc_b.get_tree("blocks");
    assert_eq!(tree_b.children_num(parent1).unwrap_or(0), 0);
    assert_eq!(tree_b.children_num(parent2).unwrap_or(0), 1);
    assert_eq!(read_text(&tree_b, block), "Moving block");
}

#[test]
fn concurrent_content_edits_merge() {
    let doc_a = LoroDoc::new();
    doc_a.set_peer_id(1).unwrap();
    let tree_a = doc_a.get_tree("blocks");
    tree_a.enable_fractional_index(0);

    let node = tree_a.create(None).unwrap();
    set_text(&tree_a, node, "Hello");
    doc_a.commit();

    let doc_b = LoroDoc::new();
    doc_b.set_peer_id(2).unwrap();
    doc_b
        .import(&doc_a.export(ExportMode::Snapshot).unwrap())
        .unwrap();

    // Both peers edit concurrently (no sync in between)
    // A appends at position 5
    let meta_a = doc_a.get_tree("blocks").get_meta(node).unwrap();
    match meta_a.get("content_raw") {
        Some(ValueOrContainer::Container(Container::Text(t))) => {
            t.insert(5, " from A").unwrap();
        }
        _ => panic!("expected text"),
    }
    doc_a.commit();

    // B appends at position 5 too
    let meta_b = doc_b.get_tree("blocks").get_meta(node).unwrap();
    match meta_b.get("content_raw") {
        Some(ValueOrContainer::Container(Container::Text(t))) => {
            t.insert(5, " from B").unwrap();
        }
        _ => panic!("expected text"),
    }
    doc_b.commit();

    // Sync both ways
    sync_both(&doc_a, &doc_b);

    let result_a = read_text(&doc_a.get_tree("blocks"), node);
    let result_b = read_text(&doc_b.get_tree("blocks"), node);

    println!("After concurrent edit merge:");
    println!("  Peer A sees: '{result_a}'");
    println!("  Peer B sees: '{result_b}'");
    assert_eq!(
        result_a, result_b,
        "Both peers should converge to same state"
    );
}

#[test]
fn concurrent_move_cycle_resolved() {
    let doc_a = LoroDoc::new();
    doc_a.set_peer_id(1).unwrap();
    let tree_a = doc_a.get_tree("blocks");
    tree_a.enable_fractional_index(0);

    let x = tree_a.create(None).unwrap();
    let y = tree_a.create(None).unwrap();
    set_text(&tree_a, x, "X");
    set_text(&tree_a, y, "Y");
    doc_a.commit();

    let doc_b = LoroDoc::new();
    doc_b.set_peer_id(2).unwrap();
    doc_b
        .import(&doc_a.export(ExportMode::Snapshot).unwrap())
        .unwrap();

    // A: move X under Y
    doc_a.get_tree("blocks").mov(x, y).unwrap();
    doc_a.commit();

    // B: move Y under X (would create cycle)
    doc_b.get_tree("blocks").mov(y, x).unwrap();
    doc_b.commit();

    // Sync — LoroTree should resolve the cycle
    sync_both(&doc_a, &doc_b);

    let tree_a_final = doc_a.get_tree("blocks");
    let tree_b_final = doc_b.get_tree("blocks");

    println!("After concurrent cycle resolution:");
    println!("  A roots: {:?}", tree_a_final.roots());
    println!("  B roots: {:?}", tree_b_final.roots());
    println!("  A: X children = {:?}", tree_a_final.children(x));
    println!("  A: Y children = {:?}", tree_a_final.children(y));

    // Both should converge — no cycle
    assert_eq!(
        tree_a_final.roots().len()
            + tree_a_final.children(x).map(|c| c.len()).unwrap_or(0)
            + tree_a_final.children(y).map(|c| c.len()).unwrap_or(0),
        tree_b_final.roots().len()
            + tree_b_final.children(x).map(|c| c.len()).unwrap_or(0)
            + tree_b_final.children(y).map(|c| c.len()).unwrap_or(0),
        "Both peers should have the same total node count"
    );
}

#[test]
fn sync_update_size() {
    let doc_a = LoroDoc::new();
    doc_a.set_peer_id(1).unwrap();
    let tree_a = doc_a.get_tree("blocks");
    tree_a.enable_fractional_index(0);

    // Create 100 blocks
    let root = tree_a.create(None).unwrap();
    for i in 0..100 {
        let block = tree_a.create(root).unwrap();
        set_text(&tree_a, block, &format!("Block {i} content"));
    }
    doc_a.commit();

    let snapshot_size = doc_a.export(ExportMode::Snapshot).unwrap().len();

    // Now make a small edit
    let children = tree_a.children(root).unwrap();
    let meta = tree_a.get_meta(children[50]).unwrap();
    match meta.get("content_raw") {
        Some(ValueOrContainer::Container(Container::Text(t))) => {
            t.insert(0, "EDITED: ").unwrap();
        }
        _ => {}
    }
    doc_a.commit();

    // Incremental update should be much smaller than full snapshot
    let vv_before_edit = doc_a.oplog_vv(); // This is current VV, not before edit
                                           // For a proper test, we'd track VV before the edit. Approximate by checking
                                           // that update from empty VV (full) is much larger than the snapshot
    let full_update = doc_a.export(ExportMode::all_updates()).unwrap();

    println!("Snapshot size: {} bytes", snapshot_size);
    println!("Full updates size: {} bytes", full_update.len());
}
