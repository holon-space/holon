//! Hypothesis: We can extract a subtree from a LoroDoc via fork -> delete non-subtree
//! nodes -> export, and the resulting doc contains usable history of the subtree.

use loro::{Container, ExportMode, LoroDoc, LoroText, LoroTree, TreeID, ValueOrContainer};
use std::collections::HashSet;

fn setup_tree_with_content(doc: &LoroDoc) -> (TreeID, TreeID, TreeID, Vec<TreeID>) {
    let tree = doc.get_tree("blocks");
    tree.enable_fractional_index(0);

    // Tree structure:
    //   doc_a (is_document)
    //     +-- heading_1
    //     |   +-- block_a
    //     |   +-- block_b
    //     +-- heading_2       <-- subtree to extract
    //         +-- block_c
    //         +-- block_d

    let doc_a = tree.create(None).unwrap();
    let meta = tree.get_meta(doc_a).unwrap();
    meta.insert("name", "doc_a").unwrap();

    let h1 = tree.create(doc_a).unwrap();
    set_text(&tree, h1, "Heading 1");
    let block_a = tree.create(h1).unwrap();
    set_text(&tree, block_a, "Block A content");
    let block_b = tree.create(h1).unwrap();
    set_text(&tree, block_b, "Block B content");

    let h2 = tree.create(doc_a).unwrap();
    set_text(&tree, h2, "Heading 2 - shared");
    let block_c = tree.create(h2).unwrap();
    set_text(&tree, block_c, "Block C content");
    let block_d = tree.create(h2).unwrap();
    set_text(&tree, block_d, "Block D content");

    let non_subtree = vec![doc_a, h1, block_a, block_b];
    (h2, block_c, block_d, non_subtree)
}

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

fn collect_subtree_ids(tree: &LoroTree, root: TreeID) -> HashSet<TreeID> {
    let mut result = HashSet::new();
    result.insert(root);
    collect_descendants(tree, root, &mut result);
    result
}

fn collect_descendants(tree: &LoroTree, node: TreeID, result: &mut HashSet<TreeID>) {
    for child in tree.children(node).unwrap_or_default() {
        result.insert(child);
        collect_descendants(tree, child, result);
    }
}

#[test]
fn fork_produces_independent_copy() {
    let doc = LoroDoc::new();
    let (h2, _block_c, _block_d, _non_subtree) = setup_tree_with_content(&doc);

    let forked = doc.fork();
    let forked_tree = forked.get_tree("blocks");
    let tree = doc.get_tree("blocks");

    assert_eq!(read_text(&tree, h2), read_text(&forked_tree, h2));

    // Editing the fork should not affect the original
    let meta = forked_tree.get_meta(h2).unwrap();
    match meta.get("content_raw") {
        Some(ValueOrContainer::Container(Container::Text(text))) => {
            text.insert(0, "[SHARED] ").unwrap();
        }
        _ => panic!("expected text container"),
    }

    assert_eq!(read_text(&forked_tree, h2), "[SHARED] Heading 2 - shared");
    assert_eq!(read_text(&tree, h2), "Heading 2 - shared");
}

#[test]
fn prune_non_subtree_nodes() {
    let doc = LoroDoc::new();
    let (h2, block_c, _block_d, non_subtree) = setup_tree_with_content(&doc);

    let forked = doc.fork();
    let forked_tree = forked.get_tree("blocks");

    for node in &non_subtree {
        forked_tree.delete(*node).unwrap();
    }

    // Subtree nodes should still be readable (even if hidden under deleted parent)
    assert_eq!(read_text(&forked_tree, h2), "Heading 2 - shared");
    assert_eq!(read_text(&forked_tree, block_c), "Block C content");

    let roots = forked_tree.roots();
    println!("Roots after pruning: {roots:?}");
    println!("h2 children: {:?}", forked_tree.children(h2));
}

#[test]
fn fork_prune_export_full_history() {
    let doc = LoroDoc::new();
    let (_h2, block_c, _block_d, non_subtree) = setup_tree_with_content(&doc);

    // Make edits to build up history
    let tree = doc.get_tree("blocks");
    let meta = tree.get_meta(block_c).unwrap();
    match meta.get("content_raw") {
        Some(ValueOrContainer::Container(Container::Text(text))) => {
            text.insert(15, " - edited once").unwrap();
            text.insert(29, " - edited twice").unwrap();
        }
        _ => panic!("expected text"),
    }

    let forked = doc.fork();
    let forked_tree = forked.get_tree("blocks");

    for node in &non_subtree {
        forked_tree.delete(*node).unwrap();
    }

    let snapshot = forked.export(ExportMode::Snapshot).unwrap();
    println!("Full snapshot size: {} bytes", snapshot.len());

    let restored = LoroDoc::new();
    restored.import(&snapshot).unwrap();
    let restored_tree = restored.get_tree("blocks");

    assert_eq!(
        read_text(&restored_tree, block_c),
        "Block C content - edited once - edited twice"
    );
}

#[test]
fn fork_prune_shallow_snapshot_trims_history() {
    let doc = LoroDoc::new();
    let (_h2, block_c, _block_d, non_subtree) = setup_tree_with_content(&doc);

    doc.commit();
    let before_edits = doc.oplog_frontiers();

    let tree = doc.get_tree("blocks");
    let meta = tree.get_meta(block_c).unwrap();
    match meta.get("content_raw") {
        Some(ValueOrContainer::Container(Container::Text(text))) => {
            text.insert(15, " - EDIT1").unwrap();
            doc.commit();
            text.insert(23, " - EDIT2").unwrap();
            doc.commit();
        }
        _ => panic!("expected text"),
    }

    let forked = doc.fork();
    let forked_tree = forked.get_tree("blocks");

    for node in &non_subtree {
        forked_tree.delete(*node).unwrap();
    }
    forked.commit();

    // Three export modes for comparison
    let full = forked.export(ExportMode::Snapshot).unwrap();
    let shallow_with_history = forked
        .export(ExportMode::shallow_snapshot(&before_edits))
        .unwrap();
    let current = forked.oplog_frontiers();
    let shallow_no_history = forked
        .export(ExportMode::shallow_snapshot(&current))
        .unwrap();

    println!("Full snapshot: {} bytes", full.len());
    println!(
        "Shallow (with edit history): {} bytes",
        shallow_with_history.len()
    );
    println!("Shallow (no history): {} bytes", shallow_no_history.len());

    // Verify content is present in all modes
    let restored = LoroDoc::new();
    restored.import(&shallow_with_history).unwrap();
    let restored_tree = restored.get_tree("blocks");
    assert_eq!(
        read_text(&restored_tree, block_c),
        "Block C content - EDIT1 - EDIT2"
    );
}

#[test]
fn subtree_visibility_after_parent_deletion() {
    // Key question: after deleting the parent of the subtree root,
    // is the subtree root visible or hidden?
    let doc = LoroDoc::new();
    let (h2, block_c, _block_d, _non_subtree) = setup_tree_with_content(&doc);

    let forked = doc.fork();
    let ft = forked.get_tree("blocks");

    let roots_before = ft.roots();
    println!("Roots before: {roots_before:?}");

    // Get parent of h2
    let doc_a = roots_before[0]; // doc_a is the only root

    // Delete doc_a — h2 is a child of doc_a, so it becomes hidden
    ft.delete(doc_a).unwrap();

    let roots_after = ft.roots();
    println!("Roots after deleting doc_a: {roots_after:?}");
    println!("h2 content still readable: {}", read_text(&ft, h2));
    println!(
        "block_c content still readable: {}",
        read_text(&ft, block_c)
    );
    println!("h2 children: {:?}", ft.children(h2));

    // If h2 is hidden, we need to reparent it before deleting doc_a.
    // Let's test the reparent-first approach:
    let forked2 = doc.fork();
    let ft2 = forked2.get_tree("blocks");

    // Move h2 to root BEFORE deleting parent
    ft2.mov(h2, None::<TreeID>).unwrap();
    ft2.delete(doc_a).unwrap();

    let roots2 = ft2.roots();
    println!("Roots after reparent+delete: {roots2:?}");
    println!("h2 is root: {}", roots2.contains(&h2));
    println!("h2 children after reparent: {:?}", ft2.children(h2));
}

#[test]
fn size_comparison_full_vs_pruned() {
    let doc = LoroDoc::new();
    let tree = doc.get_tree("blocks");
    tree.enable_fractional_index(0);

    let root = tree.create(None).unwrap();
    let meta = tree.get_meta(root).unwrap();
    meta.insert("name", "large_doc").unwrap();

    let mut target_subtree_root = None;
    for i in 0..100 {
        let heading = tree.create(root).unwrap();
        set_text(
            &tree,
            heading,
            &format!("Heading {i} with some longer title text for realism"),
        );

        if i == 50 {
            target_subtree_root = Some(heading);
        }

        for j in 0..10 {
            let block = tree.create(heading).unwrap();
            set_text(
                &tree,
                block,
                &format!("Block {i}.{j}: Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt."),
            );
        }
    }
    doc.commit();

    let full_snapshot = doc.export(ExportMode::Snapshot).unwrap();
    println!(
        "Full tree snapshot (1001 nodes): {} bytes ({:.1} KB)",
        full_snapshot.len(),
        full_snapshot.len() as f64 / 1024.0
    );

    let forked = doc.fork();
    let forked_tree = forked.get_tree("blocks");

    let subtree_root = target_subtree_root.unwrap();

    // Reparent subtree to root before deleting everything else
    forked_tree.mov(subtree_root, None::<TreeID>).unwrap();

    let keep = collect_subtree_ids(&forked_tree, subtree_root);
    let all_nodes = collect_subtree_ids(&forked_tree, root);
    for node in &all_nodes {
        if !keep.contains(node) {
            forked_tree.delete(*node).unwrap();
        }
    }
    forked.commit();

    let pruned_full = forked.export(ExportMode::Snapshot).unwrap();
    let pruned_shallow = forked
        .export(ExportMode::shallow_snapshot(&forked.oplog_frontiers()))
        .unwrap();

    println!(
        "Pruned tree (full history): {} bytes ({:.1} KB)",
        pruned_full.len(),
        pruned_full.len() as f64 / 1024.0
    );
    println!(
        "Pruned tree (shallow, no history): {} bytes ({:.1} KB)",
        pruned_shallow.len(),
        pruned_shallow.len() as f64 / 1024.0
    );
    println!(
        "Size ratio full_original / pruned_shallow: {:.1}x",
        full_snapshot.len() as f64 / pruned_shallow.len() as f64
    );
}
