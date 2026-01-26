//! Hypothesis: Shallow snapshots can be used for GC after subtree extraction.
//! After extracting a subtree into a shared tree, the parent tree can be compacted
//! via shallow_snapshot to reclaim space from the extracted content's history.

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

fn edit_text(tree: &LoroTree, node: TreeID, append: &str) {
    let meta = tree.get_meta(node).unwrap();
    match meta.get("content_raw") {
        Some(ValueOrContainer::Container(Container::Text(t))) => {
            let len = t.len_unicode();
            t.insert(len, append).unwrap();
        }
        _ => panic!("no content_raw"),
    }
}

#[test]
fn shallow_snapshot_reduces_size_after_edits() {
    let doc = LoroDoc::new();
    let tree = doc.get_tree("blocks");
    tree.enable_fractional_index(0);

    let root = tree.create(None).unwrap();
    set_text(&tree, root, "Initial");
    doc.commit();

    // Build up significant edit history
    for i in 0..100 {
        edit_text(&tree, root, &format!(" edit-{i}"));
        doc.commit();
    }

    let full = doc.export(ExportMode::Snapshot).unwrap();
    let shallow = doc
        .export(ExportMode::shallow_snapshot(&doc.oplog_frontiers()))
        .unwrap();

    println!("After 100 edits:");
    println!("  Full snapshot: {} bytes", full.len());
    println!("  Shallow (no history): {} bytes", shallow.len());
    println!(
        "  Reduction: {:.0}%",
        (1.0 - shallow.len() as f64 / full.len() as f64) * 100.0
    );

    // Verify content survived
    let restored = LoroDoc::new();
    restored.import(&shallow).unwrap();
    let restored_tree = restored.get_tree("blocks");
    let content = read_text(&restored_tree, root);
    assert!(content.starts_with("Initial"));
    assert!(content.contains("edit-99"));
}

#[test]
fn gc_parent_tree_after_extraction() {
    let doc = LoroDoc::new();
    let tree = doc.get_tree("blocks");
    tree.enable_fractional_index(0);

    let root = tree.create(None).unwrap();

    // Create two subtrees with significant content
    let kept_heading = tree.create(root).unwrap();
    set_text(&tree, kept_heading, "Kept heading");
    for i in 0..50 {
        let block = tree.create(kept_heading).unwrap();
        set_text(&tree, block, &format!("Kept block {i}: some content here"));
    }

    let extracted_heading = tree.create(root).unwrap();
    set_text(&tree, extracted_heading, "Extracted heading");
    for i in 0..50 {
        let block = tree.create(extracted_heading).unwrap();
        set_text(
            &tree,
            block,
            &format!("Extracted block {i}: content that will be removed"),
        );
        // Add some edit history
        for j in 0..5 {
            edit_text(&tree, block, &format!(" rev{j}"));
            doc.commit();
        }
    }
    doc.commit();

    let size_before = doc.export(ExportMode::Snapshot).unwrap().len();
    println!(
        "Parent tree before extraction: {} bytes ({:.1} KB)",
        size_before,
        size_before as f64 / 1024.0
    );

    // Simulate extraction: delete the extracted subtree (in real usage,
    // we'd first fork-and-prune into a shared tree, then delete from parent)
    tree.delete(extracted_heading).unwrap();
    doc.commit();

    // Replace mount node (simplified: just insert metadata on root indicating mount)
    let meta = tree.get_meta(root).unwrap();
    meta.insert("mount_0", "shared_tree_abc123").unwrap();

    let size_after_delete = doc.export(ExportMode::Snapshot).unwrap().len();
    println!(
        "After deletion (no GC): {} bytes ({:.1} KB)",
        size_after_delete,
        size_after_delete as f64 / 1024.0
    );

    // GC via shallow snapshot
    let shallow = doc
        .export(ExportMode::shallow_snapshot(&doc.oplog_frontiers()))
        .unwrap();
    println!(
        "After shallow snapshot GC: {} bytes ({:.1} KB)",
        shallow.len(),
        shallow.len() as f64 / 1024.0
    );

    // Verify kept content survived GC
    let gc_doc = LoroDoc::new();
    gc_doc.import(&shallow).unwrap();
    let gc_tree = gc_doc.get_tree("blocks");

    assert_eq!(read_text(&gc_tree, kept_heading), "Kept heading");
    assert_eq!(gc_tree.children_num(kept_heading).unwrap_or(0), 50);

    // Extracted heading should be gone (deleted)
    assert_eq!(gc_tree.children_num(root).unwrap_or(0), 1);

    let reduction = (1.0 - shallow.len() as f64 / size_before as f64) * 100.0;
    println!("Total reduction from extraction + GC: {reduction:.0}%");
}

#[test]
fn gc_preserves_sync_capability() {
    // After GC via shallow snapshot, the doc should still be syncable
    let doc_a = LoroDoc::new();
    doc_a.set_peer_id(1).unwrap();
    let tree_a = doc_a.get_tree("blocks");
    tree_a.enable_fractional_index(0);

    let root = tree_a.create(None).unwrap();
    set_text(&tree_a, root, "Root");
    for i in 0..20 {
        let block = tree_a.create(root).unwrap();
        set_text(&tree_a, block, &format!("Block {i}"));
    }
    doc_a.commit();

    // GC the doc
    let shallow = doc_a
        .export(ExportMode::shallow_snapshot(&doc_a.oplog_frontiers()))
        .unwrap();
    let gc_doc = LoroDoc::new();
    gc_doc.set_peer_id(1).unwrap();
    gc_doc.import(&shallow).unwrap();

    // Now make a new edit on the GC'd doc
    let gc_tree = gc_doc.get_tree("blocks");
    edit_text(&gc_tree, root, " (edited after GC)");
    gc_doc.commit();

    // A new peer should be able to sync from the GC'd doc
    let doc_b = LoroDoc::new();
    doc_b.set_peer_id(2).unwrap();
    let snapshot = gc_doc.export(ExportMode::Snapshot).unwrap();
    doc_b.import(&snapshot).unwrap();

    let tree_b = doc_b.get_tree("blocks");
    let content = read_text(&tree_b, root);
    assert!(content.contains("(edited after GC)"));
    println!("Sync after GC works. Content: '{content}'");
}
